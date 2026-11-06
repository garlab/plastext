use super::*;
use plastext_editor::actions::{Find, FindNext as Next, FindPrevious as Previous};
use plastext_editor::{TextInput, TextInputEvent};
use plastext_markdown::RenderedMatch;

const LIMIT: usize = 10_000;
actions!(markdown_find, [Close]);

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
    cancel: Arc<AtomicBool>,
}

impl FindState {
    pub(super) fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self._search = None;
        self.matches.clear();
        self.current = None;
        self.searching = !self.query.is_empty();
    }
}

impl Drop for FindState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl MarkdownView {
    pub(super) fn find_matches_text(&self, text: &str) -> bool {
        self.find
            .as_ref()
            .is_some_and(|f| !f.query.is_empty() && text.contains(&f.query))
    }

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
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        let input = &self.find.as_ref().unwrap().input;
        input.update(cx, |v, cx| v.select_all_text(cx));
        input.focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn search(&mut self, cx: &mut Context<Self>) {
        self.remeasure_diagrams();
        let Some(find) = &mut self.find else { return };
        find.reset();
        find.cancel = Arc::new(AtomicBool::new(false));
        find.query = find.input.read(cx).text().to_owned();
        find.searching = !find.query.is_empty();
        if !find.searching || self.parsing {
            cx.notify();
            return;
        }
        let query = find.query.clone();
        let cancel = find.cancel.clone();
        let document = self.document.clone();
        let source = self.preview_source.clone();
        let (send, receive) = async_channel::bounded(4);
        cx.background_executor()
            .spawn({
                let cancel = cancel.clone();
                let document = document.clone();
                async move {
                    let result = document.search(
                        &source,
                        &query,
                        LIMIT,
                        || cancel.load(Ordering::Relaxed),
                        |hits| send.send_blocking(Ok(hits)).is_ok(),
                    );
                    if let Err(error) = result
                        && error.kind() != std::io::ErrorKind::Interrupted
                    {
                        send.send_blocking(Err(error.to_string())).ok();
                    }
                }
            })
            .detach();
        find._search = Some(cx.spawn(async move |this, cx| {
            while let Ok(result) = receive.recv().await {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if this
                    .update(cx, |this, cx| {
                        if !Arc::ptr_eq(&document, &this.document) {
                            return;
                        }
                        let Some(find) = &mut this.find else {
                            return;
                        };
                        if !Arc::ptr_eq(&cancel, &find.cancel) {
                            return;
                        }
                        match result {
                            Ok(hits) => find.matches.extend(hits),
                            Err(error) => this.notice = Some(error.into()),
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                if let Some(find) = &mut this.find
                    && Arc::ptr_eq(&cancel, &find.cancel)
                {
                    find.searching = false;
                    cx.notify();
                }
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
        self.remeasure_diagrams();
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

    /// Find decorations are painted behind the existing glyph layout.
    pub(super) fn find_ranges(
        &self,
        text: &str,
        replacements: &[selection::Replacement],
    ) -> Vec<std::ops::Range<usize>> {
        mapped_matches(
            text,
            self.find.as_ref().map_or("", |find| find.query.as_str()),
            replacements,
        )
    }
}

fn mapped_matches(
    text: &str,
    query: &str,
    replacements: &[selection::Replacement],
) -> Vec<std::ops::Range<usize>> {
    let mut hits: Vec<std::ops::Range<usize>> = Vec::new();
    if query.is_empty() {
        return hits;
    }
    for (at, _) in text.match_indices(query) {
        let range = selection::display_range(at..at + query.len(), replacements);
        if let Some(last) = hits.last_mut().filter(|last| last.end >= range.start) {
            last.end = last.end.max(range.end);
        } else {
            hits.push(range);
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn find_maps_tex_matches_to_whole_formulas_and_keeps_plain_text_offsets() {
        let maps = [selection::Replacement {
            source: 2..5,
            display: 2..8,
        }];
        assert_eq!(
            mapped_matches("a xxx café", "x", &maps).as_slice(),
            std::slice::from_ref(&(2..8))
        );
        assert_eq!(
            mapped_matches("a xxx café", "café", &maps).as_slice(),
            std::slice::from_ref(&(9..14))
        );
        assert_eq!(
            mapped_matches("let café = 1", "et café", &[]).as_slice(),
            std::slice::from_ref(&(1..9))
        );
        assert!(mapped_matches("text", "", &[]).is_empty());
    }
}
