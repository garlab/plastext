use super::*;
use plastext_editor::{TextInput, TextInputEvent};
use plastext_markdown::{RenderedMatch, find_rendered};

const LIMIT: usize = 10_000;
actions!(markdown_find, [Find, Next, Previous, Close]);

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-f", Find, Some("MarkdownPreview")),
        KeyBinding::new("/", Find, Some("MarkdownPreview")),
        KeyBinding::new("n", Next, Some("MarkdownPreview")),
        KeyBinding::new("shift-n", Previous, Some("MarkdownPreview")),
        KeyBinding::new("cmd-g", Next, Some("MarkdownPreview")),
        KeyBinding::new("cmd-shift-g", Previous, Some("MarkdownPreview")),
        KeyBinding::new("enter", Next, Some("MarkdownFind")),
        KeyBinding::new("shift-enter", Previous, Some("MarkdownFind")),
        KeyBinding::new("escape", Close, Some("MarkdownFind")),
        KeyBinding::new("cmd-f", Find, Some("MarkdownFind")),
        KeyBinding::new("cmd-g", Next, Some("MarkdownFind")),
        KeyBinding::new("cmd-shift-g", Previous, Some("MarkdownFind")),
    ]);
}

pub(super) struct FindState {
    input: Entity<TextInput>,
    query: String,
    matches: Vec<RenderedMatch>,
    current: Option<usize>,
    searching: bool,
    _input_changes: Subscription,
    _search: Option<Task<()>>,
}

impl FindState {
    pub(super) fn reset(&mut self) {
        self._search = None;
        self.matches.clear();
        self.current = None;
        self.searching = !self.query.is_empty();
    }
}

impl MarkdownView {
    pub(super) fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| TextInput::new("Find rendered text (case sensitive)", cx));
            let changes = cx.subscribe(&input, |this, _, TextInputEvent::Changed, cx| {
                this.search(cx)
            });
            self.find = Some(FindState {
                input,
                query: String::new(),
                matches: Vec::new(),
                current: None,
                searching: false,
                _input_changes: changes,
                _search: None,
            });
        }
        let input = &self.find.as_ref().unwrap().input;
        input.update(cx, |v, cx| v.select_all_text(cx));
        input.focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn search(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        find._search = None;
        find.matches.clear();
        find.current = None;
        find.query = find.input.read(cx).text().to_owned();
        find.searching = !find.query.is_empty();
        if !find.searching {
            cx.notify();
            return;
        }
        let query = find.query.clone();
        find._search = Some(cx.spawn(async move |this, cx| {
            let mut first = 0;
            loop {
                let Ok(Some((blocks, limit))) = this.update(cx, |this, _| {
                    let find = this.find.as_ref()?;
                    if first >= this.document.blocks.len() || find.matches.len() == LIMIT {
                        return None;
                    }
                    let end = (first + 256).min(this.document.blocks.len());
                    Some((
                        this.document.blocks[first..end].to_vec(),
                        LIMIT - find.matches.len(),
                    ))
                }) else {
                    break;
                };
                let count = blocks.len();
                let query = query.clone();
                let hits = cx
                    .background_executor()
                    .spawn(async move { find_rendered(&blocks, first, &query, limit) })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if let Some(find) = &mut this.find {
                            find.matches.extend(hits);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                first += count;
                // Yield even when a short worker job completes immediately.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
            this.update(cx, |this, cx| {
                if let Some(find) = &mut this.find {
                    find.searching = false;
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn next_match(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(true, cx);
    }
    pub(super) fn previous_match(&mut self, _: &Previous, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(false, cx);
    }
    fn step_match(&mut self, next: bool, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let n = find.matches.len();
        if n == 0 {
            return;
        }
        let current = match find.current {
            None => {
                if next {
                    0
                } else {
                    n - 1
                }
            }
            Some(i) => {
                if next {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                }
            }
        };
        find.current = Some(current);
        let hit = find.matches[current].clone();
        self.reveal_metadata(hit.block);
        self.list.scroll_to(ListOffset {
            item_ix: self.visible.row(hit.block),
            offset_in_item: px(0.),
        });
        if let Some(source) = &self.source_view {
            source.update(cx, |v, cx| v.scroll_to_offset(hit.source as u64, cx));
        }
        cx.notify();
    }
    pub(super) fn close_find(&mut self, _: &Close, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn find_bar(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref().filter(|_| self.rendered)?;
        let count = find.matches.len();
        let status = format!(
            "{} / {count}{}",
            find.current.map_or(0, |i| i + 1),
            if find.searching || count == LIMIT {
                "+"
            } else {
                ""
            }
        );
        Some(
            div()
                .key_context("MarkdownFind")
                .h(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .bg(theme.surface)
                .text_color(theme.text_muted)
                .text_sm()
                .on_action(cx.listener(Self::next_match))
                .on_action(cx.listener(Self::previous_match))
                .on_action(cx.listener(Self::close_find))
                .child(div().flex_1().child(find.input.clone()))
                .child(status)
                .child(
                    div()
                        .id("md-find-previous")
                        .cursor_pointer()
                        .child("↑")
                        .on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))),
                )
                .child(
                    div()
                        .id("md-find-next")
                        .cursor_pointer()
                        .child("↓")
                        .on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))),
                )
                .child(
                    div()
                        .id("md-find-close")
                        .cursor_pointer()
                        .child("×")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_find(&Close, window, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Merge search backgrounds with existing inline styles without losing links.
    pub(super) fn highlight_matches(
        &self,
        text: &str,
        runs: Vec<TextRun>,
        theme: Theme,
    ) -> Vec<TextRun> {
        let Some(find) = &self.find else { return runs };
        if find.query.is_empty() {
            return runs;
        }
        let hits: Vec<_> = text
            .match_indices(&find.query)
            .map(|(at, _)| at..at + find.query.len())
            .collect();
        let mut result = Vec::new();
        let mut at = 0;
        let mut hit = 0;
        for run in runs {
            let end = at + run.len;
            while at < end {
                while hit < hits.len() && hits[hit].end <= at {
                    hit += 1;
                }
                let active = hits.get(hit).is_some_and(|h| h.contains(&at));
                let next = hits.get(hit).map_or(end, |h| {
                    if active {
                        h.end.min(end)
                    } else {
                        h.start.min(end)
                    }
                });
                let mut part = run.clone();
                part.len = next - at;
                if active {
                    part.background_color = Some(theme.find_match.into());
                }
                result.push(part);
                at = next;
            }
        }
        result
    }

    pub(super) fn highlight_code_matches(
        &self,
        text: &str,
        syntax: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)>,
        theme: Theme,
    ) -> Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> {
        let query = self.find.as_ref().map_or("", |find| find.query.as_str());
        code_matches(text, syntax, query, theme.find_match.into())
    }
}

fn code_matches(
    text: &str,
    syntax: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)>,
    query: &str,
    background: gpui::Hsla,
) -> Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    if query.is_empty() {
        return syntax;
    }
    gpui::combine_highlights(
        syntax,
        text.match_indices(query).map(|(at, _)| {
            (
                at..at + query.len(),
                gpui::HighlightStyle {
                    background_color: Some(background),
                    ..Default::default()
                },
            )
        }),
    )
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_find_spans_tokens_and_plain_text_without_losing_syntax_color() {
        let foreground = gpui::rgb(0x123456).into();
        let background = gpui::rgb(0xabcdef).into();
        let syntax = vec![(
            0..3,
            gpui::HighlightStyle {
                color: Some(foreground),
                ..Default::default()
            },
        )];
        let highlights = code_matches("let café = 1", syntax.clone(), "et café", background);
        assert_eq!(
            highlights
                .iter()
                .map(|(r, _)| r.clone())
                .collect::<Vec<_>>(),
            [0..1, 1..3, 3..9]
        );
        assert_eq!(highlights[0].1.color, Some(foreground));
        assert_eq!(highlights[0].1.background_color, None);
        assert_eq!(highlights[1].1.color, Some(foreground));
        assert_eq!(highlights[1].1.background_color, Some(background));
        assert_eq!(highlights[2].1.background_color, Some(background));
        assert_eq!(
            code_matches("let café = 1", syntax, "", background).len(),
            1
        );
    }
}
