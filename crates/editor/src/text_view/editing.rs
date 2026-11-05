use super::*;
use crate::EditingDocument;
use gpui::{ElementInputHandler, EntityInputHandler, UTF16Selection};
use plastext_buffer::{
    Bias, CursorCommand, Direction, PaneId, PendingCommand, Selection as BufferSelection,
    SessionError, TextError, UndoGroup,
};
use std::collections::VecDeque;

actions!(
    editing,
    [
        EnterEdit,
        LeaveEdit,
        Save,
        Undo,
        Redo,
        Backspace,
        Delete,
        InsertNewline,
        InsertTab,
        Paste,
        Cut,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocumentStart,
        DocumentEnd,
        SelectDocumentStart,
        SelectDocumentEnd
    ]
);
const CONTEXT: &str = "EditingText";

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("e", EnterEdit, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", EnterEdit, Some(KEY_CONTEXT)),
    ]);
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", LeaveEdit, context),
        KeyBinding::new("cmd-s", Save, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("enter", InsertNewline, context),
        KeyBinding::new("tab", InsertTab, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-f", Find, context),
        KeyBinding::new("cmd-g", FindNext, context),
        KeyBinding::new("cmd-shift-g", FindPrevious, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("cmd-left", LineStart, context),
        KeyBinding::new("ctrl-a", LineStart, context),
        KeyBinding::new("cmd-right", LineEnd, context),
        KeyBinding::new("ctrl-e", LineEnd, context),
        KeyBinding::new("cmd-shift-left", SelectLineStart, context),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, context),
        KeyBinding::new("cmd-up", DocumentStart, context),
        KeyBinding::new("cmd-down", DocumentEnd, context),
        KeyBinding::new("cmd-shift-up", SelectDocumentStart, context),
        KeyBinding::new("cmd-shift-down", SelectDocumentEnd, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("pagedown", PageDown, context),
    ]);
}

#[derive(Clone)]
enum Input {
    Text {
        text: String,
        range: Option<Range<usize>>,
        marked: bool,
        selected: Option<Range<usize>>,
        separate: bool,
    },
    Cursor(CursorCommand),
    Vertical(i32, bool),
    Edge(bool, bool),
    DocumentEdge(bool, bool),
    Selection(BufferSelection),
    Newline,
    Tab,
    Undo,
    Redo,
    Save,
}

#[derive(Clone)]
struct QueuedInput {
    input: Arc<Input>,
    coordinates: Option<u64>,
}

pub(super) struct Editing {
    document: Entity<EditingDocument>,
    pane: Option<PaneId>,
    active: bool,
    dirty: bool,
    generation: Option<u64>,
    marked: Option<Range<u64>>,
    typing: Option<Instant>,
    queue: VecDeque<QueuedInput>,
    reveal_caret: bool,
    document_notice: Option<SharedString>,
    cursor: Option<PendingCommand>,
    retry: Option<Task<()>>,
    goal_column: Option<usize>,
    _observer: Subscription,
}

impl Editing {
    pub(super) fn release(self, cx: &mut App) {
        self.document.update(cx, |document, _| {
            document.views = document.views.saturating_sub(1);
            document.pending_inputs = document.pending_inputs.saturating_sub(self.queue.len());
            if let (Some(file), Some(pane)) = (&mut document.file, self.pane) {
                file.session_mut().detach_pane(pane);
            }
        });
    }
}

impl TextView {
    pub fn set_editing_document(
        &mut self,
        document: Entity<EditingDocument>,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() {
            return;
        }
        document.update(cx, |document, _| document.views += 1);
        let observer = cx.observe(&document, |this, _, cx| {
            this.sync_editing(cx);
            this.drain_input(cx);
        });
        self.editing = Some(Editing {
            document,
            pane: None,
            active: false,
            dirty: false,
            generation: None,
            marked: None,
            typing: None,
            queue: VecDeque::new(),
            reveal_caret: false,
            document_notice: None,
            cursor: None,
            retry: None,
            goal_column: None,
            _observer: observer,
        });
        self.sync_editing(cx);
    }
    pub fn editing_document(&self) -> Option<Entity<EditingDocument>> {
        self.editing
            .as_ref()
            .map(|editing| editing.document.clone())
    }
    pub fn is_dirty(&self) -> bool {
        self.editing.as_ref().is_some_and(|editing| editing.dirty)
    }
    pub fn is_editing(&self) -> bool {
        self.editing.as_ref().is_some_and(|editing| editing.active)
    }
    pub fn has_pending_input(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|editing| !editing.queue.is_empty())
    }
    pub fn enter_edit(&mut self, _: &EnterEdit, _: &mut Window, cx: &mut Context<Self>) {
        self.begin_edit(cx);
    }
    pub(super) fn begin_edit(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.editing_document() else {
            return;
        };
        let desired = self.selection.unwrap_or(Selection {
            anchor: self.top,
            head: self.top,
        });
        document.update(cx, |document, cx| document.begin_edit(desired.head, cx));
        self.sync_editing(cx);
        if self.is_editing() {
            self.queue_input(
                Input::Selection(BufferSelection::new(desired.anchor, desired.head)),
                cx,
            );
        }
    }
    fn leave_edit(&mut self, _: &LeaveEdit, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(editing) = &mut self.editing {
            if !editing.queue.is_empty() {
                return;
            }
            editing.marked = None;
            editing
                .document
                .update(cx, |document, cx| document.leave_edit(cx));
        }
    }
    pub(super) fn sync_editing(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &mut self.editing else {
            return;
        };
        let desired = self
            .selection
            .map_or(BufferSelection::caret(self.top), |value| {
                BufferSelection::new(value.anchor, value.head)
            });
        let state = editing.document.update(cx, |document, _| {
            let Some(file) = &mut document.file else {
                return None;
            };
            let session = file.session_mut();
            let snapshot = session.snapshot();
            let pane = (|| {
                if editing.pane.is_none() {
                    let len = snapshot.text().len();
                    let anchor = snapshot
                        .text()
                        .clip_to_scalar(desired.anchor.offset.min(len), Bias::Before)?;
                    let head = snapshot
                        .text()
                        .clip_to_scalar(desired.head.offset.min(len), Bias::After)?;
                    editing.pane = Some(session.attach_pane(BufferSelection::new(anchor, head))?);
                }
                session.pane_snapshot(editing.pane.unwrap())
            })();
            Some((snapshot, pane, document.active))
        });
        if state.is_none() {
            editing.active = editing.document.read(cx).preparing;
        }
        let notice = editing.document.read(cx).notice.clone();
        editing.dirty = editing.document.read(cx).is_dirty();
        let mut missing = None;
        if let Some((snapshot, pane, active)) = state {
            let changed = editing.generation != Some(snapshot.generation());
            editing.active = active;
            if changed {
                editing.generation = Some(snapshot.generation());
                // A different pane's edit invalidates composition coordinates.
                editing.marked = None;
                editing.cursor = None;
                self.cancel_parse.store(true, Ordering::Relaxed);
                self._parse = None;
                self._index_refresh = None;
                self.syntax = None;
                self.syntax_folds = None;
                self.symbols = None;
                self.folds = Folds::default();
                // The private editing original replaces the reader cache; do not
                // retain a second full small-file copy or large-file block cache.
                let original = Arc::new(FileSource::from_bytes(Bytes::new()));
                let old = std::mem::replace(
                    &mut self.source,
                    Arc::new(ViewSource {
                        original,
                        snapshot: Some(snapshot.text().clone()),
                    }),
                );
                cx.background_executor()
                    .spawn(async move {
                        drop(old);
                    })
                    .detach();
                self.top = self.top.min(self.source.len());
                self.pending_jump = Some(Jump::Offset(self.top));
                self.loading = None;
                self.error = None;
            }
            match pane {
                Ok(pane) => {
                    let selection = pane.selection();
                    self.selection = Some(Selection {
                        anchor: selection.anchor.offset,
                        head: selection.head.offset,
                    });
                }
                Err(SessionError::Text(TextError::Read(ReadError::Missing(range)))) => {
                    missing = Some(range)
                }
                Err(error) => {
                    self.notice = Some(format!("Couldn't restore selection: {error:?}").into())
                }
            }
            if changed && self.find.is_some() {
                self.search(cx);
            }
        }
        let editing = self.editing.as_mut().unwrap();
        if editing.document_notice != notice {
            self.notice = notice.clone();
            editing.document_notice = notice;
        }
        if let Some(range) = missing {
            self.load(range, cx);
        }
        cx.notify();
    }
    fn queue_input(&mut self, input: Input, cx: &mut Context<Self>) {
        if !self.is_editing() {
            return;
        }
        self.editing
            .as_ref()
            .unwrap()
            .document
            .update(cx, |document, _| document.pending_inputs += 1);
        let editing = self.editing.as_mut().unwrap();
        let positional = matches!(
            &input,
            Input::Selection(_)
                | Input::Text { range: Some(_), .. }
                | Input::Text { marked: true, .. }
        ) || matches!(&input, Input::Text { .. }) && editing.marked.is_some();
        editing.queue.push_back(QueuedInput {
            input: Arc::new(input),
            coordinates: positional.then_some(editing.generation).flatten(),
        });
        self.drain_input(cx);
    }
    pub(super) fn drain_input(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &self.editing else {
            return;
        };
        if editing.queue.is_empty() || editing.document.read(cx).preparing {
            return;
        }
        if editing.document.read(cx).file.is_none() {
            self.fail_pending_input(cx);
            return;
        }
        for _ in 0..32 {
            let Some(input) = self
                .editing
                .as_ref()
                .and_then(|editing| editing.queue.front())
                .cloned()
            else {
                return;
            };
            if input.coordinates.is_some()
                && input.coordinates != self.editing.as_ref().unwrap().generation
            {
                self.editing.as_mut().unwrap().queue.pop_front();
                self.input_finished(cx);
                self.notice = Some("The document changed while input was loading. Please repeat the selection or composition.".into());
                cx.notify();
                continue;
            }
            match self.apply_input(&input.input, cx) {
                Ok(()) => {
                    self.editing.as_mut().unwrap().queue.pop_front();
                    self.input_finished(cx);
                }
                Err(SessionError::Text(TextError::Read(ReadError::Missing(range)))) => {
                    self.load(range, cx);
                    return;
                }
                Err(SessionError::Text(TextError::Edit(plastext_buffer::EditError::Read(
                    ReadError::Missing(range),
                )))) => {
                    self.load(range, cx);
                    return;
                }
                Err(SessionError::Pending | SessionError::Stale) => {
                    self.retry_input(cx);
                    return;
                }
                Err(error) => {
                    let editing = self.editing.as_mut().unwrap();
                    editing.queue.pop_front();
                    editing.cursor = None;
                    self.input_finished(cx);
                    self.notice = Some(format!("Couldn't edit: {error:?}").into());
                    cx.notify();
                }
            }
        }
        self.retry_input(cx);
    }
    fn input_finished(&self, cx: &mut Context<Self>) {
        self.editing
            .as_ref()
            .unwrap()
            .document
            .update(cx, |document, cx| {
                document.pending_inputs = document.pending_inputs.saturating_sub(1);
                cx.notify();
            });
    }
    fn retry_input(&mut self, cx: &mut Context<Self>) {
        if self.editing.as_ref().unwrap().retry.is_some() {
            return;
        }
        self.editing.as_mut().unwrap().retry = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
            this.update(cx, |this, cx| {
                if let Some(editing) = &mut this.editing {
                    editing.retry = None;
                }
                this.drain_input(cx);
            })
            .ok();
        }));
    }
    fn apply_input(&mut self, input: &Input, cx: &mut Context<Self>) -> Result<(), SessionError> {
        let editing = self.editing.as_mut().unwrap();
        let document = editing.document.clone();
        if matches!(input, Input::Save) {
            document.update(cx, |document, cx| document.save(cx));
            return Ok(());
        }
        let pane_id = editing.pane.ok_or(SessionError::Pending)?;
        let source = self.source.clone();
        let tab_size = self.style.tab_size;
        let result = document.update(cx, |document, cx| {
            let session = document
                .file
                .as_mut()
                .ok_or(SessionError::Pending)?
                .session_mut();
            let base = session.pane_snapshot(pane_id)?;
            let snapshot = base.document().text();
            match input {
                Input::Selection(selection) => {
                    let selection = BufferSelection::new(
                        snapshot.clip_to_scalar(
                            selection.anchor.offset.min(snapshot.len()),
                            Bias::Before,
                        )?,
                        snapshot.clip_to_scalar(
                            selection.head.offset.min(snapshot.len()),
                            Bias::After,
                        )?,
                    );
                    session.set_selection(&base, selection)?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Cursor(command) => {
                    if editing.cursor.is_none() {
                        editing.cursor = Some(session.cursor_command(&base, *command)?);
                    }
                    let pending = editing.cursor.as_mut().unwrap();
                    if !pending.poll(1)? {
                        return Err(SessionError::Pending);
                    }
                    let result = session.finish_command(pending);
                    editing.cursor = None;
                    result?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Undo => {
                    session.undo(&base)?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Redo => {
                    session.redo(&base)?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Newline => {
                    session.insert_newline(&base)?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Tab => {
                    session.replace(&base, base.selection().range(), "\t", UndoGroup::New)?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Vertical(delta, extend) => {
                    let head = base.selection().head.offset;
                    let row = source.row_start(head).map_err(TextError::Read)?;
                    let end = source.row_end(row).map_err(TextError::Read)?;
                    let bytes = snapshot.bytes(row..end).map_err(TextError::Read)?;
                    let goal = *editing
                        .goal_column
                        .get_or_insert_with(|| column_at(&bytes, tab_size, (head - row) as usize));
                    let target = if *delta < 0 {
                        source.previous(&Folds::default(), row)
                    } else {
                        source.next(&Folds::default(), row)
                    }
                    .map_err(TextError::Read)?
                    .unwrap_or(row);
                    let end = source.row_end(target).map_err(TextError::Read)?;
                    let bytes = snapshot.bytes(target..end).map_err(TextError::Read)?;
                    let content = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                    let content = content.strip_suffix(b"\r").unwrap_or(content);
                    let target = target + offset_at(content, tab_size, goal) as u64;
                    session.set_selection(
                        &base,
                        BufferSelection::new(
                            if *extend {
                                base.selection().anchor.offset
                            } else {
                                target
                            },
                            target,
                        ),
                    )?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::DocumentEdge(end, extend) => {
                    let target = if *end { snapshot.len() } else { 0 };
                    session.set_selection(
                        &base,
                        BufferSelection::new(
                            if *extend {
                                base.selection().anchor.offset
                            } else {
                                target
                            },
                            target,
                        ),
                    )?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Edge(end, extend) => {
                    let line = snapshot
                        .line_at(base.selection().head.offset)
                        .map_err(TextError::Read)?;
                    let mut target = snapshot
                        .line_start(line + u64::from(*end))
                        .map_err(TextError::Read)?
                        .unwrap_or(snapshot.len());
                    if *end && target > 0 {
                        let bytes = snapshot
                            .bytes(target.saturating_sub(2)..target)
                            .map_err(TextError::Read)?;
                        if bytes.ends_with(b"\r\n") {
                            target -= 2;
                        } else if bytes.ends_with(b"\r") || bytes.ends_with(b"\n") {
                            target -= 1;
                        }
                    }
                    session.set_selection(
                        &base,
                        BufferSelection::new(
                            if *extend {
                                base.selection().anchor.offset
                            } else {
                                target
                            },
                            target,
                        ),
                    )?;
                    editing.marked = None;
                    editing.typing = None;
                }
                Input::Text {
                    text,
                    range,
                    marked,
                    selected,
                    separate,
                } => {
                    let range = if let Some(range) = range {
                        snapshot.utf16_to_byte(
                            (range.start as u64).min(snapshot.utf16_len()),
                            Bias::Before,
                        )?
                            ..snapshot.utf16_to_byte(
                                (range.end as u64).min(snapshot.utf16_len()),
                                Bias::After,
                            )?
                    } else {
                        editing
                            .marked
                            .clone()
                            .unwrap_or_else(|| base.selection().range())
                    };
                    let group = if !separate
                        && (editing.marked.is_some()
                            || editing
                                .typing
                                .is_some_and(|time| time.elapsed() < Duration::from_millis(600)))
                    {
                        UndoGroup::Continue
                    } else {
                        UndoGroup::New
                    };
                    session.replace(&base, range.clone(), text, group)?;
                    editing.marked = marked.then_some(range.start..range.start + text.len() as u64);
                    editing.typing = if *separate {
                        None
                    } else {
                        Some(Instant::now())
                    };
                    if *marked && let Some(selected) = selected {
                        let bytes = |units: usize| {
                            let mut utf16 = 0;
                            for (at, ch) in text.char_indices() {
                                if utf16 >= units {
                                    return at as u64;
                                }
                                utf16 += ch.len_utf16();
                            }
                            text.len() as u64
                        };
                        let base = session.pane_snapshot(pane_id)?;
                        session.set_composition_selection(
                            &base,
                            BufferSelection::new(
                                range.start + bytes(selected.start),
                                range.start + bytes(selected.end),
                            ),
                        )?;
                    }
                }
                Input::Save => unreachable!(),
            }
            if !matches!(input, Input::Vertical(..)) {
                editing.goal_column = None;
            }
            document.changed(cx);
            Ok(())
        });
        // Pull the changed tree now, before the next queued keystroke or paint.
        if result.is_ok() {
            let marked = self.editing.as_ref().unwrap().marked.clone();
            self.sync_editing(cx);
            self.editing.as_mut().unwrap().marked = marked;
            self.editing.as_mut().unwrap().reveal_caret = true;
            if let Some(selection) = self.selection {
                self.jump(Jump::Reveal(selection.head), cx);
            }
        }
        result
    }
    pub(super) fn selection_edited(&mut self, cx: &mut Context<Self>) {
        if self.is_editing()
            && let Some(selection) = self.selection
        {
            self.queue_input(
                Input::Selection(BufferSelection::new(selection.anchor, selection.head)),
                cx,
            );
        }
    }
    pub(super) fn fail_pending_input(&mut self, cx: &mut Context<Self>) {
        if let Some(editing) = &mut self.editing {
            let count = editing.queue.len();
            editing.queue.clear();
            editing.cursor = None;
            editing.document.update(cx, |document, cx| {
                document.pending_inputs = document.pending_inputs.saturating_sub(count);
                if count > 0 {
                    let reason = document
                        .notice
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "Couldn't load the edit location.".to_owned());
                    document.notice = Some(
                        format!(
                            "{reason} Pending input was not applied; existing changes are retained."
                        )
                        .into(),
                    );
                }
                cx.notify();
            });
        }
    }
    pub(super) fn reveal_caret_x(&mut self, width: Pixels) {
        if !self
            .editing
            .as_ref()
            .is_some_and(|editing| editing.reveal_caret)
        {
            return;
        }
        let Some(selection) = self.selection else {
            return;
        };
        let Ok(start) = self.source.row_start(selection.head) else {
            return;
        };
        let Ok(bytes) = self.source.bytes(start..selection.head) else {
            return;
        };
        let x = self.char_width * column_at(&bytes, self.style.tab_size, bytes.len()) as f32;
        if x < self.scroll_x {
            self.scroll_x = x;
        }
        if x + self.char_width > self.scroll_x + width {
            self.scroll_x = (x + self.char_width - width).max(px(0.));
        }
        self.editing.as_mut().unwrap().reveal_caret = false;
    }
    fn edit_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.queue_input(
                Input::Text {
                    text,
                    range: None,
                    marked: false,
                    selected: None,
                    separate: true,
                },
                cx,
            );
        }
    }
    fn edit_cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_pending_input() {
            self.notice = Some("Wait for the pending selection before cutting.".into());
            cx.notify();
            return;
        }
        let Some(selection) = self
            .selection
            .filter(|selection| !selection.range().is_empty())
        else {
            return;
        };
        if selection.range().end - selection.range().start > MAX_COPY {
            self.notice = Some("The selection is too large to cut.".into());
            return;
        }
        // Never remove text until its clipboard read has completed.
        if self.source.bytes(selection.range()).is_err() {
            self.load(selection.range(), cx);
            self.notice = Some("Loading selection; press Cut again when ready.".into());
            return;
        }
        self.copy(&Copy, window, cx);
        self.queue_input(
            Input::Text {
                text: String::new(),
                range: None,
                marked: false,
                selected: None,
                separate: true,
            },
            cx,
        );
    }
    pub(super) fn editing_actions(&self, element: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        element
            .key_context(if self.is_editing() {
                CONTEXT
            } else {
                KEY_CONTEXT
            })
            .on_action(cx.listener(|this, _: &DocumentStart, _, cx| {
                this.queue_input(Input::DocumentEdge(false, false), cx)
            }))
            .on_action(cx.listener(|this, _: &DocumentEnd, _, cx| {
                this.queue_input(Input::DocumentEdge(true, false), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDocumentStart, _, cx| {
                this.queue_input(Input::DocumentEdge(false, true), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDocumentEnd, _, cx| {
                this.queue_input(Input::DocumentEdge(true, true), cx)
            }))
            .on_action(cx.listener(Self::enter_edit))
            .on_action(cx.listener(Self::leave_edit))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.queue_input(Input::Save, cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.queue_input(Input::Undo, cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.queue_input(Input::Redo, cx)))
            .on_action(
                cx.listener(|this, _: &InsertNewline, _, cx| this.queue_input(Input::Newline, cx)),
            )
            .on_action(cx.listener(|this, _: &InsertTab, _, cx| this.queue_input(Input::Tab, cx)))
            .on_action(cx.listener(Self::edit_paste))
            .on_action(cx.listener(Self::edit_cut))
            .on_action(cx.listener(|this, _: &Backspace, _, cx| {
                this.queue_input(
                    Input::Cursor(CursorCommand::Delete {
                        direction: Direction::Backward,
                    }),
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                this.queue_input(
                    Input::Cursor(CursorCommand::Delete {
                        direction: Direction::Forward,
                    }),
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &Left, _, cx| {
                this.queue_input(
                    Input::Cursor(CursorCommand::Move {
                        direction: Direction::Backward,
                        extend: false,
                    }),
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &Right, _, cx| {
                this.queue_input(
                    Input::Cursor(CursorCommand::Move {
                        direction: Direction::Forward,
                        extend: false,
                    }),
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
                this.queue_input(
                    Input::Cursor(CursorCommand::Move {
                        direction: Direction::Backward,
                        extend: true,
                    }),
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| {
                this.queue_input(
                    Input::Cursor(CursorCommand::Move {
                        direction: Direction::Forward,
                        extend: true,
                    }),
                    cx,
                )
            }))
            .on_action(
                cx.listener(|this, _: &Up, _, cx| this.queue_input(Input::Vertical(-1, false), cx)),
            )
            .on_action(
                cx.listener(|this, _: &Down, _, cx| {
                    this.queue_input(Input::Vertical(1, false), cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| {
                this.queue_input(Input::Vertical(-1, true), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| {
                this.queue_input(Input::Vertical(1, true), cx)
            }))
            .on_action(cx.listener(|this, _: &LineStart, _, cx| {
                this.queue_input(Input::Edge(false, false), cx)
            }))
            .on_action(cx.listener(|this, _: &LineEnd, _, cx| {
                this.queue_input(Input::Edge(true, false), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLineStart, _, cx| {
                this.queue_input(Input::Edge(false, true), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLineEnd, _, cx| {
                this.queue_input(Input::Edge(true, true), cx)
            }))
    }
    pub(super) fn paint_input(
        &mut self,
        hits: &HitMap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_hits = Some(hits.clone());
        if !self.is_editing() {
            return;
        }
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(hits.bounds, cx.entity()),
            cx,
        );
        if let Some(marked) = self
            .editing
            .as_ref()
            .and_then(|editing| editing.marked.as_ref())
        {
            for row in &hits.rows {
                let start = marked.start.max(row.start);
                let end = marked.end.min(row.start + row.content.len() as u64);
                if start < end
                    && let (Some(a), Some(b)) = (hits.caret(start), hits.caret(end))
                {
                    let underline = Bounds::new(
                        point(a.left(), a.bottom() - px(2.)),
                        size((b.left() - a.left()).max(px(1.)), px(1.)),
                    );
                    window.with_content_mask(
                        Some(ContentMask {
                            bounds: Bounds::from_corners(
                                point(hits.bounds.left() + hits.gutter_width, hits.bounds.top()),
                                hits.bounds.bottom_right(),
                            ),
                        }),
                        |window| window.paint_quad(fill(underline, Theme::get(window, cx).text)),
                    );
                }
            }
        }
        if self.focus_handle.is_focused(window)
            && let Some(selection) = self.selection
            && let Some(bounds) = hits.caret(selection.head)
        {
            window.with_content_mask(
                Some(ContentMask {
                    bounds: Bounds::from_corners(
                        point(hits.bounds.left() + hits.gutter_width, hits.bounds.top()),
                        hits.bounds.bottom_right(),
                    ),
                }),
                |window| {
                    window.paint_quad(fill(bounds, Theme::get(window, cx).text));
                },
            );
        }
    }
}

impl HitMap {
    fn caret(&self, offset: u64) -> Option<Bounds<Pixels>> {
        let i = self.rows.iter().rposition(|row| row.start <= offset)?;
        let row = &self.rows[i];
        if offset > row.start + row.content.len() as u64 {
            return None;
        }
        let column = column_at(&row.content, self.tab_size, (offset - row.start) as usize);
        Some(Bounds::new(
            point(
                self.bounds.left()
                    + self.text_left
                    + self.char_width * (column as f32 - self.first_column as f32),
                self.bounds.top() - self.top_offset + self.line_height * i as f32,
            ),
            size(px(1.5), self.line_height),
        ))
    }
}

impl EntityInputHandler for TextView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if range.start > range.end {
            return None;
        }
        let text = self.source.snapshot.as_ref()?;
        let start = text
            .utf16_to_byte((range.start as u64).min(text.utf16_len()), Bias::Before)
            .ok()?;
        let end = text
            .utf16_to_byte(
                (range.end.min(range.start.saturating_add(65536)) as u64).min(text.utf16_len()),
                Bias::After,
            )
            .ok()?;
        let bytes = match text.bytes(start..end) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.handle_read_error(error, cx);
                return None;
            }
        };
        *actual =
            Some(text.byte_to_utf16(start).ok()? as usize..text.byte_to_utf16(end).ok()? as usize);
        Some(input_text(&bytes))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let text = self.source.snapshot.as_ref()?;
        let selection = self.selection?;
        let range = selection.range();
        Some(UTF16Selection {
            range: text.byte_to_utf16(range.start).ok()? as usize
                ..text.byte_to_utf16(range.end).ok()? as usize,
            reversed: selection.anchor > selection.head,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let text = self.source.snapshot.as_ref()?;
        let marked = self.editing.as_ref()?.marked.as_ref()?;
        Some(
            text.byte_to_utf16(marked.start).ok()? as usize
                ..text.byte_to_utf16(marked.end).ok()? as usize,
        )
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if let Some(editing) = &mut self.editing {
            editing.marked = None;
            editing.typing = None;
        }
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue_input(
            Input::Text {
                text: text.to_owned(),
                range,
                marked: false,
                selected: None,
                separate: false,
            },
            cx,
        );
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue_input(
            Input::Text {
                text: text.to_owned(),
                range,
                marked: !text.is_empty(),
                selected,
                separate: false,
            },
            cx,
        );
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let text = self.source.snapshot.as_ref()?;
        let start = text
            .utf16_to_byte((range.start as u64).min(text.utf16_len()), Bias::Before)
            .ok()?;
        self.last_hits.as_ref()?.caret(start)
    }
    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let hit = self.last_hits.as_ref()?.hit(point)?;
        Some(
            self.source
                .snapshot
                .as_ref()?
                .byte_to_utf16(hit.offset)
                .ok()? as usize,
        )
    }
}

// The buffer indexes each malformed byte as one replacement character.
fn input_text(bytes: &[u8]) -> String {
    let mut text = String::new();
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        text.extend(std::iter::repeat_n('\u{fffd}', chunk.invalid().len()));
    }
    text
}

#[cfg(test)]
#[path = "editing_tests.rs"]
mod tests;
