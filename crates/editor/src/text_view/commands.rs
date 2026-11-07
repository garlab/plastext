use super::*;
use plastext_buffer::{DocumentSession, PaneSnapshot, TextEdit, TextSnapshot};
use plastext_settings::Editor;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) enum Command {
    Occurrence(bool),
    Insert(String, bool),
    Paste(String),
    Newline,
    Tab,
    Indent,
    Outdent,
    Comment,
    Move(bool),
    Duplicate(bool),
}

fn read(text: &TextSnapshot, range: Range<u64>) -> Result<String, SessionError> {
    if range.end - range.start > MAX_COPY {
        return Err(SessionError::Limit("Command exceeds the 64 MB text limit"));
    }
    String::from_utf8({
        let mut bytes = Vec::new();
        text.read_uncached(range, &mut bytes)
            .map_err(|_| SessionError::Limit("Could not read command input"))?;
        bytes
    })
    .map_err(|_| {
        SessionError::Limit(
            "This command would rewrite invalid text bytes; select valid text first",
        )
    })
}
fn line_range(text: &TextSnapshot, line: u64) -> Result<Range<u64>, SessionError> {
    Ok(
        loaded(text, || text.line_start(line))?.unwrap_or(text.len())..loaded(text, || {
            text.line_start(line + 1)
        })?
        .unwrap_or(text.len()),
    )
}
fn leading(s: &str) -> &str {
    &s[..s.bytes().take_while(|b| matches!(b, b' ' | b'\t')).count()]
}
fn indent(options: &Editor, columns: usize) -> String {
    if options.insert_spaces {
        " ".repeat(columns)
    } else {
        "\t".repeat(columns / options.tab_size.clamp(1, 16))
            + &" ".repeat(columns % options.tab_size.clamp(1, 16))
    }
}
fn selected_lines(
    text: &TextSnapshot,
    selections: &[BufferSelection],
) -> Result<Vec<(u64, u64)>, SessionError> {
    let mut lines = BTreeSet::new();
    for selection in selections {
        let r = selection.range();
        let first = loaded(text, || text.line_at(r.start))?;
        let last = loaded(text, || {
            text.line_at(if r.is_empty() { r.end } else { r.end - 1 })
        })?;
        if last - first > 100_000 {
            return Err(SessionError::Limit(
                "Select at most 100,000 lines for a line command",
            ));
        }
        lines.extend(first..=last);
        if lines.len() > 100_000 {
            return Err(SessionError::Limit("Select at most 100,000 lines"));
        }
    }
    let mut blocks: Vec<(u64, u64)> = Vec::new();
    for line in lines {
        match blocks.last_mut() {
            Some((_, end)) if *end + 1 == line => *end = line,
            _ => blocks.push((line, line)),
        }
    }
    Ok(blocks)
}

pub(super) fn plan(
    base: &PaneSnapshot,
    command: Command,
    options: &Editor,
    language: Option<&Language>,
) -> Result<Plan, SessionError> {
    if let Command::Paste(ref text) = command {
        return insertion_plan(base, text, true);
    }
    if let Command::Insert(ref text, separate) = command {
        if options.auto_close_pairs
            && !separate
            && let Some(prepared) = super::super::pairs::prepare(base, text, true)?
        {
            return Ok(Plan {
                prepared,
                group: UndoGroup::New,
            });
        }
        return insertion_plan(base, text, false);
    }
    if let Command::Occurrence(all) = command {
        return occurrence_plan(base, all);
    }
    let text = base.document().text();
    let selections = base.selections();
    let unit = options.indent_size.clamp(1, 16);
    let mut edits = Vec::new();
    let mut carets = Vec::new();
    let mut desired = None;
    let tab_indent =
        matches!(command, Command::Tab) && selections.iter().any(|s| !s.range().is_empty());
    if matches!(command, Command::Newline | Command::Tab) && !tab_indent {
        for selection in &selections {
            let range = selection.range();
            let row = loaded(text, || text.line_at(range.start))?;
            let start = line_range(text, row)?.start;
            let before = read(text, start..range.start.min(start + 65536))?;
            let inserted = if matches!(command, Command::Tab) {
                if options.insert_spaces {
                    " ".repeat(
                        unit - column_at(before.as_bytes(), options.tab_size, before.len()) % unit,
                    )
                } else {
                    "\t".into()
                }
            } else {
                let newline = text.preferred_line_ending().as_str();
                let prefix = if options.auto_indent {
                    leading(&before)
                } else {
                    ""
                };
                let extra =
                    options.auto_indent && before.trim_end().ends_with(['{', '[', '(', ':']);
                format!(
                    "{newline}{prefix}{}",
                    if extra {
                        indent(options, unit)
                    } else {
                        String::new()
                    }
                )
            };
            carets.push(BufferSelection::caret(range.end));
            edits.push(TextEdit {
                range,
                text: inserted,
            });
        }
    } else {
        let blocks = selected_lines(text, &selections)?;
        let mut total = 0u64;
        for (first, last) in &blocks {
            total += line_range(text, *last)?.end - line_range(text, *first)?.start;
        }
        if total > MAX_COPY {
            return Err(SessionError::Limit("Line command exceeds 64 MB"));
        }
        match command {
            Command::Indent | Command::Outdent | Command::Tab | Command::Comment => {
                let mut lines = Vec::new();
                let mut total = 0;
                for (first, last) in blocks {
                    for line in first..=last {
                        let range = line_range(text, line)?;
                        total += range.end - range.start;
                        if total > MAX_COPY {
                            return Err(SessionError::Limit("Line command exceeds 64 MB"));
                        }
                        let content = read(text, range.clone())?;
                        lines.push((range, content));
                    }
                }
                let marker = language.and_then(|l| l.lexical.line_comments.first().copied());
                let block_marker = match language.map(|l| l.name) {
                    Some("HTML" | "XML" | "Markdown") => Some(("<!--", "-->")),
                    Some("CSS" | "SCSS") => Some(("/*", "*/")),
                    _ => None,
                };
                if matches!(command, Command::Comment) && marker.is_none() && block_marker.is_none()
                {
                    return Err(SessionError::Limit("This language has no comment syntax"));
                }
                let uncomment = matches!(command, Command::Comment)
                    && lines
                        .iter()
                        .filter(|(_, s)| !s.trim().is_empty())
                        .all(|(_, s)| {
                            s.trim_start()
                                .starts_with(marker.unwrap_or_else(|| block_marker.unwrap().0))
                        });
                for (range, content) in lines {
                    let prefix = leading(&content);
                    let at = range.start + prefix.len() as u64;
                    match command {
                        Command::Indent | Command::Tab => edits.push(TextEdit {
                            range: range.start..range.start,
                            text: indent(options, unit),
                        }),
                        Command::Outdent => {
                            let count = if prefix.starts_with('\t') {
                                1
                            } else {
                                prefix.len().min(unit)
                            };
                            edits.push(TextEdit {
                                range: range.start..range.start + count as u64,
                                text: String::new(),
                            });
                        }
                        Command::Comment if !content.trim().is_empty() => {
                            let (open, close) = marker
                                .map(|m| (m, ""))
                                .unwrap_or_else(|| block_marker.unwrap());
                            if uncomment {
                                let after = &content[prefix.len() + open.len()..];
                                let count = open.len() + usize::from(after.starts_with(' '));
                                edits.push(TextEdit {
                                    range: at..at + count as u64,
                                    text: String::new(),
                                });
                                if !close.is_empty() {
                                    let body = content.trim_end();
                                    if let Some(stripped) = body.strip_suffix(close) {
                                        let n =
                                            stripped.len() - usize::from(stripped.ends_with(' '));
                                        edits.push(TextEdit {
                                            range: range.start + n as u64
                                                ..range.start + body.len() as u64,
                                            text: String::new(),
                                        });
                                    }
                                }
                            } else {
                                edits.push(TextEdit {
                                    range: at..at,
                                    text: format!("{open} "),
                                });
                                if !close.is_empty() {
                                    let end = range.start
                                        + content.trim_end_matches(['\r', '\n']).len() as u64;
                                    edits.push(TextEdit {
                                        range: end..end,
                                        text: format!(" {close}"),
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Command::Move(down) | Command::Duplicate(down) => {
                let mut moved = selections.clone();
                let mut owners = vec![None; selections.len()];
                for (first, last) in blocks {
                    let start = line_range(text, first)?.start;
                    let end = line_range(text, last)?.end;
                    let block = read(text, start..end)?;
                    let newline = text.preferred_line_ending().as_str();
                    let (range, replacement, target) = if matches!(command, Command::Duplicate(_)) {
                        if down {
                            let copy = if block.ends_with(['\r', '\n']) {
                                block.clone()
                            } else {
                                format!("{newline}{block}")
                            };
                            let target = end
                                + if block.ends_with(['\r', '\n']) {
                                    0
                                } else {
                                    newline.len() as u64
                                };
                            (end..end, copy, target)
                        } else {
                            let copy = if block.ends_with(['\r', '\n']) {
                                block.clone()
                            } else {
                                format!("{block}{newline}")
                            };
                            (start..start, copy, start)
                        }
                    } else if down {
                        if last + 1 >= text.line_count() || end == text.len() {
                            continue;
                        }
                        let next = line_range(text, last + 1)?;
                        let adjacent = read(text, next.clone())?;
                        let replacement = swap(&block, &adjacent, newline);
                        let target = start
                            + adjacent.len() as u64
                            + if adjacent.ends_with(['\r', '\n']) {
                                0
                            } else {
                                newline.len() as u64
                            };
                        (start..next.end, replacement, target)
                    } else {
                        if first == 0 {
                            continue;
                        }
                        let prev = line_range(text, first - 1)?;
                        let adjacent = read(text, prev.clone())?;
                        (
                            prev.start..end,
                            swap(&adjacent, &block, newline),
                            prev.start,
                        )
                    };
                    for (i, (original, selection)) in selections.iter().zip(&mut moved).enumerate()
                    {
                        if original.range().start >= start
                            && (original.range().start < end || start == end)
                        {
                            owners[i] = Some(edits.len());
                            *selection = BufferSelection::new(
                                target + original.anchor.offset - start,
                                target + original.head.offset - start,
                            );
                        }
                    }
                    edits.push(TextEdit {
                        range,
                        text: replacement,
                    });
                }
                // Targets are in the replacement's coordinates; only other
                // transactions before that replacement shift their final position.
                for (i, selection) in moved.iter_mut().enumerate() {
                    if let Some(owner) = owners[i] {
                        let shift: i64 = edits
                            .iter()
                            .enumerate()
                            .filter(|(j, e)| {
                                *j != owner && e.range.start < edits[owner].range.start
                            })
                            .map(|(_, e)| {
                                e.text.len() as i64 - (e.range.end - e.range.start) as i64
                            })
                            .sum();
                        selection.anchor.offset =
                            selection.anchor.offset.saturating_add_signed(shift);
                        selection.head.offset = selection.head.offset.saturating_add_signed(shift);
                    } else {
                        for edit in edits.iter().rev() {
                            selection.rebase(&plastext_buffer::EditDelta {
                                range: edit.range.clone(),
                                inserted: edit.text.len() as u64,
                            });
                        }
                    }
                }
                let output_len = (text.len() as i64
                    + edits
                        .iter()
                        .map(|e| e.text.len() as i64 - (e.range.end - e.range.start) as i64)
                        .sum::<i64>()) as u64;
                for s in &mut moved {
                    s.anchor.offset = s.anchor.offset.min(output_len);
                    s.head.offset = s.head.offset.min(output_len);
                }
                desired = Some(moved);
            }
            _ => unreachable!(),
        }
    }
    if edits.iter().map(|e| e.text.len()).sum::<usize>() > MAX_COPY as usize {
        return Err(SessionError::Limit("Command exceeds 64 MB"));
    }
    let prepared = text.prepare_edits_loading(&edits, &AtomicBool::new(false))?;
    if !carets.is_empty() {
        let mut ordered: Vec<_> = edits.iter().collect();
        ordered.sort_by_key(|e| e.range.start);
        for caret in &mut carets {
            for edit in ordered.iter().rev() {
                caret.rebase(&plastext_buffer::EditDelta {
                    range: edit.range.clone(),
                    inserted: edit.text.len() as u64,
                });
            }
        }
        desired = Some(carets);
    }
    let prepared = if let Some(desired) = desired {
        prepared.with_selections(desired, &AtomicBool::new(false))?
    } else {
        prepared
    };
    Ok(Plan {
        prepared,
        group: UndoGroup::New,
    })
}

pub(super) struct Plan {
    group: UndoGroup,
    prepared: plastext_buffer::PreparedEdits,
}
impl Plan {
    pub(super) fn install(
        self,
        session: &mut DocumentSession,
        base: &PaneSnapshot,
    ) -> Result<(), SessionError> {
        session.apply_prepared(base, self.prepared, self.group)?;
        Ok(())
    }
}

fn swap(a: &str, b: &str, newline: &str) -> String {
    if b.ends_with(['\r', '\n']) {
        format!("{b}{a}")
    } else {
        format!(
            "{b}{newline}{}",
            a.strip_suffix("\r\n")
                .or_else(|| a.strip_suffix(['\r', '\n']))
                .unwrap_or(a)
        )
    }
}

impl TextView {
    pub(in crate::text_view) fn all_selections(&self) -> Vec<Selection> {
        if let Some(editing) = &self.editing
            && editing.active
            && editing.selections.len() > 1
        {
            return editing
                .selections
                .iter()
                .map(|s| Selection {
                    anchor: s.anchor.offset,
                    head: s.head.offset,
                })
                .collect();
        }
        self.selection.into_iter().collect()
    }
    pub(in crate::text_view) fn add_cursor(&mut self, offset: u64, cx: &mut Context<Self>) {
        let mut selections: Vec<_> = self
            .all_selections()
            .into_iter()
            .map(|s| BufferSelection::new(s.anchor, s.head))
            .collect();
        if let Some(at) = selections
            .iter()
            .position(|s| s.range().is_empty() && s.head.offset == offset)
        {
            if selections.len() > 1 {
                selections.remove(at);
            }
        } else {
            selections.insert(0, BufferSelection::caret(offset));
        }
        self.queue_input(Input::Selections(selections), cx);
    }
}

impl TextView {
    pub(super) fn command_actions(&self, element: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        use crate::actions::*;
        let can_reopen = self.editing.as_ref().is_some_and(|e| {
            !e.document.read(cx).is_dirty()
                && !e.document.read(cx).is_saving()
                && !e.document.read(cx).is_preparing()
        });
        element.when(self.is_editing(), |element| {
            element
                .on_action(cx.listener(Self::open_replace))
                .when(can_reopen, |element| {
                    element.on_action(cx.listener(|this, action: &ReopenEncoding, _, cx| {
                        if let Some(editing) = &this.editing {
                            editing
                                .document
                                .update(cx, |doc, cx| doc.reopen_encoding(&action.label, cx));
                        }
                    }))
                })
                .on_action(cx.listener(|this, _: &JumpToBracket, _, cx| {
                    if let Some((_, target)) = this.matching_brackets() {
                        this.queue_input(Input::Selection(BufferSelection::caret(target)), cx);
                        this.jump(Jump::Caret(target), cx);
                    }
                }))
                .on_action(cx.listener(|this, action: &SetEncoding, _, cx| {
                    if let Some(text) = &this.source.snapshot
                        && let Some(encoding) =
                            plastext_buffer::TextEncoding::for_label(&action.label)
                        && let Ok(format) = plastext_buffer::TextFormat::new(encoding, action.bom)
                    {
                        let format = format.with_line_ending(text.format().line_ending());
                        this.queue_input(Input::Format(format, true), cx);
                    }
                }))
                .on_action(cx.listener(|this, action: &SetLineEnding, _, cx| {
                    if let Some(text) = &this.source.snapshot {
                        let ending = match action.ending.as_str() {
                            "lf" => Some(plastext_buffer::LineEnding::Lf),
                            "crlf" => Some(plastext_buffer::LineEnding::CrLf),
                            "cr" => Some(plastext_buffer::LineEnding::Cr),
                            _ => None,
                        };
                        this.queue_input(
                            Input::Format(text.format().with_line_ending(ending), false),
                            cx,
                        );
                    }
                }))
                .on_action(cx.listener(|this, _: &ToggleWhitespace, _, cx| {
                    if let Some(editing) = &this.editing {
                        editing.document.update(cx, |doc, cx| {
                            doc.preferences.show_whitespace = !doc.preferences.show_whitespace;
                            cx.notify();
                        });
                    }
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &AddNextOccurrence, _, cx| {
                    this.queue_input(Input::Command(Command::Occurrence(false)), cx)
                }))
                .on_action(cx.listener(|this, _: &SelectAllOccurrences, _, cx| {
                    this.queue_input(Input::Command(Command::Occurrence(true)), cx)
                }))
                .on_action(cx.listener(|this, _: &AddCursorAbove, _, cx| {
                    this.queue_input(Input::AddVertical(-1), cx)
                }))
                .on_action(cx.listener(|this, _: &AddCursorBelow, _, cx| {
                    this.queue_input(Input::AddVertical(1), cx)
                }))
                .on_action(cx.listener(|this, _: &Indent, _, cx| {
                    this.queue_input(Input::Command(Command::Indent), cx)
                }))
                .on_action(cx.listener(|this, _: &Outdent, _, cx| {
                    this.queue_input(Input::Command(Command::Outdent), cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleComment, _, cx| {
                    this.queue_input(Input::Command(Command::Comment), cx)
                }))
                .on_action(cx.listener(|this, _: &MoveLineUp, _, cx| {
                    this.queue_input(Input::Command(Command::Move(false)), cx)
                }))
                .on_action(cx.listener(|this, _: &MoveLineDown, _, cx| {
                    this.queue_input(Input::Command(Command::Move(true)), cx)
                }))
                .on_action(cx.listener(|this, _: &DuplicateLineUp, _, cx| {
                    this.queue_input(Input::Command(Command::Duplicate(false)), cx)
                }))
                .on_action(cx.listener(|this, _: &DuplicateLineDown, _, cx| {
                    this.queue_input(Input::Command(Command::Duplicate(true)), cx)
                }))
        })
    }
}

pub(super) fn init(cx: &mut App) {
    use crate::actions::*;
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-alt-f", Replace, context),
        KeyBinding::new("cmd-alt-f", Replace, Some(FIND_KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-\\", JumpToBracket, context),
        KeyBinding::new("cmd-alt-g", AddNextOccurrence, context),
        KeyBinding::new("cmd-shift-l", SelectAllOccurrences, context),
        KeyBinding::new("cmd-alt-up", AddCursorAbove, context),
        KeyBinding::new("cmd-alt-down", AddCursorBelow, context),
        KeyBinding::new("cmd-]", Indent, context),
        KeyBinding::new("cmd-[", Outdent, context),
        KeyBinding::new("shift-tab", Outdent, context),
        KeyBinding::new("cmd-/", ToggleComment, context),
        KeyBinding::new("alt-up", MoveLineUp, context),
        KeyBinding::new("alt-down", MoveLineDown, context),
        KeyBinding::new("alt-shift-up", DuplicateLineUp, context),
        KeyBinding::new("alt-shift-down", DuplicateLineDown, context),
    ]);
}

impl TextView {
    pub(super) fn start_line_command(&mut self, command: Command, cx: &mut Context<Self>) {
        let editing = self.editing.as_ref().unwrap();
        let document = editing.document.clone();
        let Some(base) = document
            .read(cx)
            .file
            .as_ref()
            .and_then(|f| f.session().pane_snapshot(editing.pane?).ok())
        else {
            self.retry_input(cx);
            return;
        };
        let group = if matches!(command, Command::Insert(_, false))
            && editing
                .typing
                .is_some_and(|t| t.elapsed() < Duration::from_millis(600))
        {
            UndoGroup::Continue
        } else {
            UndoGroup::New
        };
        let options = document.read(cx).preferences.clone();
        let language = self.language;
        let work = cx.background_executor().spawn(async move {
            let result = loop {
                match plan(&base, command.clone(), &options, language) {
                    Err(SessionError::Text(TextError::Read(ReadError::Missing(range)))) => {
                        if base.document().text().load(range).is_err() {
                            break Err(SessionError::Limit("Could not load command input"));
                        }
                    }
                    result => break result,
                }
            };
            (
                base,
                result.map(|mut plan| {
                    plan.group = group;
                    plan
                }),
            )
        });
        self.editing.as_mut().unwrap().bulk = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if let Some(editing) = &mut this.editing {
                    editing.bulk = None;
                    editing.bulk_result = Some(result);
                }
                this.drain_input(cx);
            })
            .ok();
        }));
    }
}

fn loaded<T>(
    text: &TextSnapshot,
    mut f: impl FnMut() -> Result<T, ReadError>,
) -> Result<T, SessionError> {
    loop {
        match f() {
            Err(ReadError::Missing(range)) => text
                .load(range)
                .map_err(|_| SessionError::Limit("Could not read command input"))?,
            result => return result.map_err(|e| SessionError::Text(TextError::Read(e))),
        }
    }
}

fn occurrence_plan(base: &PaneSnapshot, all: bool) -> Result<Plan, SessionError> {
    let text = base.document().text();
    let mut selections = base.selections();
    let primary = selections[0].range();
    if primary.is_empty() {
        let clip = |at, bias| {
            loaded(text, || {
                text.clip_to_scalar(at, bias).map_err(|e| match e {
                    TextError::Read(r) => r,
                    _ => ReadError::Changed,
                })
            })
        };
        let start = clip(primary.start.saturating_sub(32768), Bias::After)?;
        let end = clip((primary.start + 32768).min(text.len()), Bias::Before)?;
        let bytes = read(text, start..end)?;
        let at = (primary.start - start) as usize;
        if at > bytes.len() || !bytes.is_char_boundary(at) {
            return Err(SessionError::Limit("Word is too large to select"));
        }
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let left = bytes[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| word(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let right = at
            + bytes[at..]
                .char_indices()
                .take_while(|(_, c)| word(*c))
                .last()
                .map_or(0, |(i, c)| i + c.len_utf8());
        if left == 0 && start > 0 || right == bytes.len() && end < text.len() {
            return Err(SessionError::Limit("Word is too large to select"));
        }
        if left == right {
            return Err(SessionError::Limit("Select text to add occurrences"));
        }
        selections[0] = BufferSelection::new(start + left as u64, start + right as u64);
        if !all {
            return selection_plan(base, selections);
        }
    }
    let pattern = read(text, selections[0].range())?;
    let query = DocumentQuery::new(
        &pattern,
        SearchOptions {
            case_sensitive: true,
            ..Default::default()
        },
    )
    .map_err(|_| SessionError::Limit("Selection is too large to search"))?;
    let mut found = Vec::new();
    let mut first = None;
    let mut next = None;
    query
        .scan(
            text.len(),
            |r, out| text.read_uncached(r, out),
            0..text.len(),
            None,
            &AtomicBool::new(false),
            |range, _| {
                if selections.iter().any(|s| s.range() == range) {
                    return true;
                }
                if all {
                    found.push(range);
                    return found.len() <= 4096;
                }
                if range.start >= selections[0].range().end {
                    next = Some(range);
                    return false;
                }
                first.get_or_insert(range);
                true
            },
        )
        .map_err(|_| SessionError::Limit("Could not search occurrences"))?;
    if all {
        if found.len() + selections.len() > 4096 {
            return Err(SessionError::Limit(
                "At most 4096 cursors are supported; narrow the selection",
            ));
        }
        selections.extend(
            found
                .into_iter()
                .map(|r| BufferSelection::new(r.start, r.end)),
        );
    } else if let Some(next) = next.or(first) {
        selections.insert(0, BufferSelection::new(next.start, next.end));
    }
    selection_plan(base, selections)
}
fn selection_plan(
    base: &PaneSnapshot,
    selections: Vec<BufferSelection>,
) -> Result<Plan, SessionError> {
    Ok(Plan {
        group: UndoGroup::New,
        prepared: base
            .document()
            .text()
            .prepare_edits(&[])?
            .with_selections(selections, &AtomicBool::new(false))?,
    })
}

impl TextView {
    pub(in crate::text_view) fn matching_brackets(&self) -> Option<(u64, u64)> {
        if self.syntax_map.is_some() {
            return None;
        }
        let offset = self.selection?.head;
        self.syntax
            .as_ref()?
            .matching_brackets(offset as usize)
            .map(|(a, b)| (a as u64, b as u64))
    }
}

fn insertion_plan(
    base: &PaneSnapshot,
    input: &str,
    distribute: bool,
) -> Result<Plan, SessionError> {
    let mut selections = base.selections();
    let primary = selections[0];
    selections.sort_by_key(|s| s.range().start);
    let primary_index = selections.iter().position(|s| *s == primary).unwrap();
    let lines: Vec<_> = input
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .collect();
    let distribute = distribute && selections.len() > 1 && lines.len() == selections.len();
    if input
        .len()
        .saturating_mul(if distribute { 1 } else { selections.len() })
        > MAX_COPY as usize
    {
        return Err(SessionError::Limit("Insertion exceeds 64 MB"));
    }
    let mut shift = 0i64;
    let mut targets = Vec::new();
    let edits: Vec<_> = selections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let range = s.range();
            let text = if distribute { lines[i] } else { input };
            targets.push(BufferSelection::caret(
                range.start.saturating_add_signed(shift) + text.len() as u64,
            ));
            shift += text.len() as i64 - (range.end - range.start) as i64;
            TextEdit {
                range,
                text: text.into(),
            }
        })
        .collect();
    targets.rotate_left(primary_index);
    let cancel = AtomicBool::new(false);
    Ok(Plan {
        group: UndoGroup::New,
        prepared: base
            .document()
            .text()
            .prepare_edits_loading(&edits, &cancel)?
            .with_selections(targets, &cancel)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(
        source: &str,
        selections: Vec<BufferSelection>,
        command: Command,
        language: Option<&'static Language>,
    ) -> (String, Vec<BufferSelection>) {
        let mut session =
            DocumentSession::new(plastext_buffer::EditBuffer::from_bytes(source.to_owned()));
        let pane = session.attach_pane(selections[0]).unwrap();
        session
            .set_selections(&session.pane_snapshot(pane).unwrap(), selections)
            .unwrap();
        let base = session.pane_snapshot(pane).unwrap();
        plan(&base, command, &Editor::default(), language)
            .unwrap()
            .install(&mut session, &base)
            .unwrap();
        let now = session.pane_snapshot(pane).unwrap();
        let content = String::from_utf8(
            now.document()
                .text()
                .bytes(0..now.document().text().len())
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        session.undo(&now).unwrap();
        let text = session.snapshot();
        assert_eq!(
            text.text().bytes(0..text.text().len()).unwrap().as_ref(),
            source.as_bytes()
        );
        (content, now.selections())
    }
    #[test]
    fn duplicate_multiple_blocks_tracks_each_copy_and_undo() {
        let (text, selections) = run(
            "a\nb\nc\nd",
            vec![BufferSelection::caret(0), BufferSelection::caret(4)],
            Command::Duplicate(true),
            None,
        );
        assert_eq!(text, "a\na\nb\nc\nc\nd");
        assert_eq!(
            selections,
            vec![BufferSelection::caret(2), BufferSelection::caret(8)]
        );
        let (text, selections) = run(
            "a\nb\nc\nd",
            vec![BufferSelection::caret(0), BufferSelection::caret(4)],
            Command::Duplicate(false),
            None,
        );
        assert_eq!(text, "a\na\nb\nc\nc\nd");
        assert_eq!(
            selections,
            vec![BufferSelection::caret(0), BufferSelection::caret(6)]
        );
    }
    #[test]
    fn moving_crlf_and_unterminated_final_lines_preserves_selection() {
        let (text, selections) = run(
            "aaa\r\nbb\r\nç",
            vec![BufferSelection::new(8, 5)],
            Command::Move(true),
            None,
        );
        assert_eq!(text, "aaa\r\nç\r\nbb");
        // Selection included the original CR, which no longer exists on the final line.
        assert!(selections[0].head.offset >= 9);
        let (text, _) = run(
            "a\r\nb",
            vec![BufferSelection::caret(3)],
            Command::Move(false),
            None,
        );
        assert_eq!(text, "b\r\na");
    }
    #[test]
    fn indent_excludes_the_line_at_selection_end_and_newline_inherits_indent() {
        let (text, _) = run(
            "a\nb\nc\n",
            vec![BufferSelection::new(0, 4)],
            Command::Indent,
            None,
        );
        assert_eq!(text, "    a\n    b\nc\n");
        let (text, selection) = run(
            "  {",
            vec![BufferSelection::caret(3)],
            Command::Newline,
            None,
        );
        assert_eq!(text, "  {\n      ");
        assert_eq!(selection, vec![BufferSelection::caret(10)]);
    }
    #[test]
    fn next_occurrence_is_not_limited_by_earlier_matches() {
        let mut session = DocumentSession::new(plastext_buffer::EditBuffer::from_bytes(
            "foo ".repeat(10000),
        ));
        let pane = session
            .attach_pane(BufferSelection::new(30000, 30003))
            .unwrap();
        let base = session.pane_snapshot(pane).unwrap();
        occurrence_plan(&base, false)
            .unwrap()
            .install(&mut session, &base)
            .unwrap();
        assert_eq!(
            session.pane_snapshot(pane).unwrap().selection(),
            BufferSelection::new(30004, 30007)
        );
    }
    #[test]
    fn occurrence_selection_can_wrap_and_avoids_duplicates() {
        let mut session =
            DocumentSession::new(plastext_buffer::EditBuffer::from_bytes("foo bar foo foo"));
        let pane = session.attach_pane(BufferSelection::new(8, 11)).unwrap();
        let base = session.pane_snapshot(pane).unwrap();
        occurrence_plan(&base, false)
            .unwrap()
            .install(&mut session, &base)
            .unwrap();
        assert_eq!(
            session.pane_snapshot(pane).unwrap().selections(),
            vec![BufferSelection::new(12, 15), BufferSelection::new(8, 11)]
        );
        let base = session.pane_snapshot(pane).unwrap();
        occurrence_plan(&base, true)
            .unwrap()
            .install(&mut session, &base)
            .unwrap();
        assert_eq!(session.pane_snapshot(pane).unwrap().selections().len(), 3);
    }
}
