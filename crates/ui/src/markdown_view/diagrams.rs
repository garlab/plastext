//! Viewport-driven Mermaid and SGF diagrams with one budget across windows and tabs.
use super::*;
use gpui::{AnyWindowHandle, BorrowAppContext, EntityId, Global, RenderImage, img};

const BUDGET: usize = 32 * 1024 * 1024;
type Key = (EntityId, usize, bool);
#[derive(Clone)]
struct Diagram {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    caption: Option<SharedString>,
}
struct Entry {
    result: Result<Diagram, String>,
    window: AnyWindowHandle,
    bytes: usize,
    used: u64,
    pinned: bool,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    clock: u64,
}
impl Global for Cache {}
impl Cache {
    fn get(key: Key, cx: &mut App) -> Option<Result<Diagram, String>> {
        if !cx.has_global::<Self>() {
            return None;
        }
        cx.update_global::<Self, _>(|cache, _| {
            cache.clock += 1;
            for (other, entry) in &mut cache.entries {
                if other.0 == key.0 && other.1 == key.1 {
                    entry.pinned = *other == key;
                }
            }
            if !cache.entries.contains_key(&key)
                && cache.entries.len() >= 128
                && cache.entries.values().all(|e| e.pinned)
            {
                return Some(Err("Diagram cache full · use Source".into()));
            }
            let entry = cache.entries.get_mut(&key)?;
            entry.used = cache.clock;
            entry.pinned = true;
            Some(entry.result.clone())
        })
    }
    fn insert(
        key: Key,
        result: Result<(Diagram, usize), String>,
        window: AnyWindowHandle,
        cx: &mut App,
    ) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
        cx.update_global::<Self, _>(|cache, cx| {
            cache.remove_where(|k, _| *k == key, cx);
            let bytes = result.as_ref().map_or(0, |(_, bytes)| *bytes);
            // Count both CPU pixels and GPU atlas storage. Never evict another
            // pane's visible image to satisfy this one: that causes render loops.
            while cache.bytes + bytes > BUDGET || cache.entries.len() >= 128 {
                let oldest = cache
                    .entries
                    .iter()
                    .filter(|(_, e)| !e.pinned)
                    .min_by_key(|(_, e)| e.used)
                    .map(|(k, _)| *k);
                let Some(oldest) = oldest else {
                    break;
                };
                cache.remove_where(|k, _| *k == oldest, cx);
            }
            let (result, bytes) = if cache.bytes + bytes > BUDGET {
                (
                    Err("Diagram cache full · use Source or retry Preview".into()),
                    0,
                )
            } else {
                (result.map(|(diagram, _)| diagram), bytes)
            };
            // Failed previews are cheap but also count towards the entry limit.
            if cache.entries.len() >= 128 {
                return;
            }
            cache.clock += 1;
            cache.bytes += bytes;
            cache.entries.insert(
                key,
                Entry {
                    result,
                    window,
                    bytes,
                    used: cache.clock,
                    pinned: true,
                },
            );
        });
    }
    fn remove_where(&mut self, predicate: impl Fn(&Key, &Entry) -> bool, cx: &mut App) {
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter(|(k, e)| predicate(k, e))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            let entry = self.entries.remove(&key).unwrap();
            self.bytes -= entry.bytes;
            if let Ok(Diagram { image, .. }) = entry.result {
                cx.defer(move |cx| {
                    let _ = entry
                        .window
                        .update(cx, |_, window, _| window.drop_image(image));
                });
            }
        }
    }
    fn remove(owner: EntityId, block: Option<usize>, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|cache, cx| {
                cache.remove_where(|k, _| k.0 == owner && block.is_none_or(|b| b == k.1), cx);
            });
        }
    }
    fn retry(owner: EntityId, block: usize, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|cache, cx| {
                cache.remove_where(
                    |k, entry| k.0 == owner && k.1 == block && entry.result.is_err(),
                    cx,
                );
            });
        }
    }
    fn begin_frame(owner: EntityId, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|cache, _| {
                for (key, entry) in &mut cache.entries {
                    if key.0 == owner {
                        entry.pinned = false;
                    }
                }
            });
        }
    }
}

#[derive(Default)]
pub(super) struct Diagrams {
    source: HashSet<usize>,
    rows: HashSet<usize>,
    pending: HashMap<usize, Arc<AtomicBool>>,
    pub(super) hidden: bool,
}
impl Diagrams {
    pub(super) fn reset_source_choices(&mut self) {
        self.source.clear();
    }
    pub fn begin_frame(&mut self, owner: EntityId, cx: &mut App) -> bool {
        self.rows.clear();
        Cache::begin_frame(owner, cx);
        !self.pending.is_empty()
    }
    pub fn clear(&mut self, owner: EntityId, cx: &mut App) {
        for (_, cancel) in self.pending.drain() {
            cancel.store(true, Ordering::Relaxed);
        }
        Cache::remove(owner, None, cx);
        self.rows.clear();
    }
    pub fn finish_frame(&mut self) -> bool {
        let before = self.pending.len();
        self.pending.retain(|ix, cancel| {
            if self.rows.contains(ix) {
                true
            } else {
                cancel.store(true, Ordering::Relaxed);
                false
            }
        });
        self.pending.len() != before
    }
}
impl Drop for Diagrams {
    fn drop(&mut self) {
        for cancel in self.pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Mermaid,
    Sgf,
}
impl Kind {
    fn for_language(language: &str, mermaid_enabled: bool) -> Option<Self> {
        if language.eq_ignore_ascii_case("mermaid") && mermaid_enabled {
            Some(Self::Mermaid)
        } else if language.eq_ignore_ascii_case("sgf") {
            Some(Self::Sgf)
        } else {
            None
        }
    }
    fn render(
        self,
        source: &str,
        attributes: &str,
        light: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(document_graphics::Raster, Option<String>), String> {
        match self {
            Self::Mermaid => plastext_platform::mermaid::render(source, light, cancel)
                .map_err(|e| e.to_string())?
                .map(|pixels| (pixels, None))
                .ok_or_else(|| "Unsupported diagram content or preview limits exceeded".into()),
            Self::Sgf => sgf::render(source, attributes, light, cancel)
                .map(|(pixels, caption)| (pixels, Some(caption)))
                .map_err(|e| e.to_string()),
        }
    }
}
fn complete_fence(document: &DocumentIndex, ix: usize) -> bool {
    document.blocks.get(ix).is_some_and(|b| b.code_first)
        && document
            .blocks
            .get(ix + 1)
            .is_none_or(|b| b.kind != plastext_markdown::BlockType::Code || b.code_first)
}

fn preview_rejection(
    document: &DocumentIndex,
    ix: usize,
    source: &str,
    kind: Kind,
) -> Option<String> {
    if !complete_fence(document, ix) {
        Some(format!(
            "Diagram exceeds the preview limit ({} lines or 16 KiB) · showing source",
            plastext_markdown::DIAGRAM_LINE_LIMIT
        ))
    } else if kind == Kind::Mermaid && !document_graphics::mermaid::admissible(source) {
        Some("Unsupported diagram syntax or preview limits exceeded · showing source".into())
    } else {
        None
    }
}

impl MarkdownView {
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let changed = self.diagrams.hidden == visible;
        self.diagrams.hidden = !visible;
        if changed {
            self.refresh_preview(visible, cx);
        }
        if !visible {
            self.diagrams.clear(cx.entity_id(), cx);
            self.clear_images(cx);
        }
    }

    pub(super) fn code_block(
        &mut self,
        block: Arc<Block>,
        ix: usize,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let BlockKind::Code {
            language,
            attributes,
            first,
        } = &block.kind
        else {
            unreachable!()
        };
        let first = *first;
        let kind = first
            .then(|| Kind::for_language(language, self.settings.mermaid))
            .flatten();
        if kind.is_some() {
            self.diagrams.rows.insert(ix);
        }
        let found = self.find_matches_text(&block.text);
        let source = self.diagrams.source.contains(&ix) || found;
        let light =
            theme.background.r * 0.2126 + theme.background.g * 0.7152 + theme.background.b * 0.0722
                > 0.5;
        let key = (cx.entity_id(), ix, light);
        let result = if let Some(kind) = kind.filter(|_| !source) {
            if self.parsing {
                None
            } else if let Some(reason) = preview_rejection(&self.document, ix, &block.text, kind) {
                Some(Err(reason))
            } else {
                let result = Cache::get(key, cx);
                if result.is_none()
                    && !self.diagrams.hidden
                    && !self.diagrams.pending.contains_key(&ix)
                    && self.diagrams.pending.len() < 2
                {
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.diagrams.pending.insert(ix, cancel.clone());
                    let window = window.window_handle();
                    let text = block.text.clone();
                    let attributes = attributes.clone();
                    let task = cx.background_executor().spawn({
                        let cancel = cancel.clone();
                        async move {
                            kind.render(&text, &attributes, light, &cancel)
                                .map_err(|e| format!("{e} · showing source"))
                                .map(|(preview, caption)| {
                                    let bytes = preview.rgba.len() * 2;
                                    let (image, width, height) =
                                        crate::image_view::render_image(plastext_assets::Preview {
                                            width: preview.width,
                                            height: preview.height,
                                            original_width: preview.original_width,
                                            original_height: preview.original_height,
                                            rgba: preview.rgba,
                                        });
                                    (
                                        Diagram {
                                            image,
                                            width,
                                            height,
                                            caption: caption.map(Into::into),
                                        },
                                        bytes,
                                    )
                                })
                        }
                    });
                    cx.spawn(async move |this, cx| {
                        let result = task.await;
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let _ = this.update(cx, |this, cx| {
                            if !this
                                .diagrams
                                .pending
                                .get(&ix)
                                .is_some_and(|c| Arc::ptr_eq(c, &cancel))
                            {
                                return;
                            }
                            this.diagrams.pending.remove(&ix);
                            Cache::insert(key, result, window, cx);
                            this.remeasure_diagram(ix, cx);
                        });
                    })
                    .detach();
                }
                result
            }
        } else {
            None
        };
        let content = if kind.is_some() && !source {
            match result {
                Some(Ok(Diagram {
                    image,
                    width,
                    height,
                    caption,
                })) => div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(width as f32 * self.zoom))
                            .map(|mut d| {
                                d.style().aspect_ratio = Some(width as f32 / height as f32);
                                d
                            })
                            .child(img(image).size_full()),
                    )
                    .when_some(caption, |d, caption| {
                        d.child(div().mt_2().text_color(theme.text_muted).child(caption))
                    })
                    .into_any_element(),
                Some(Err(error)) => div()
                    .child(div().mb_2().text_color(theme.text_muted).child(error))
                    .child(self.code_text(block.clone(), ix, language, theme, cx))
                    .into_any_element(),
                None => div()
                    .h(px(96.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_muted)
                    .child("Rendering diagram…")
                    .into_any_element(),
            }
        } else {
            self.code_text(block.clone(), ix, language, theme, cx)
        };
        div()
            .p_3()
            .rounded_md()
            .bg(theme.surface)
            .text_size(px(13. * self.zoom))
            .when(first, |d| {
                d.child(
                    div()
                        .flex()
                        .justify_between()
                        .mb_2()
                        .text_size(px(12. * self.zoom))
                        .text_color(theme.text_muted)
                        .child(language.to_owned())
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .when(kind.is_some() && found, |d| d.child("Source · find"))
                                .when(kind.is_some() && !found, |d| {
                                    d.child(
                                        div()
                                            .id(("md-diagram-mode", ix))
                                            .debug_selector(|| "md-diagram-mode".into())
                                            .cursor_pointer()
                                            .child(if source { "Preview" } else { "Source" })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if !this.diagrams.source.remove(&ix) {
                                                    this.diagrams.source.insert(ix);
                                                }
                                                if let Some(cancel) =
                                                    this.diagrams.pending.remove(&ix)
                                                {
                                                    cancel.store(true, Ordering::Relaxed);
                                                }
                                                // Keep a successful raster for the return to
                                                // Preview. A short loading placeholder can
                                                // clamp the scroll position near the document
                                                // end before the tall diagram renders again.
                                                // Source leaves it unpinned and evictable.
                                                Cache::retry(cx.entity_id(), ix, cx);
                                                this.remeasure_diagram(ix, cx);
                                                this.focus.focus(window);
                                            })),
                                    )
                                })
                                .child(
                                    div()
                                        .id(("md-copy", ix))
                                        .debug_selector(|| "md-copy".into())
                                        .cursor_pointer()
                                        .child(if self.parsing { "Loading…" } else { "Copy" })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.parsing {
                                                this.copy_fence(ix, cx);
                                            }
                                        })),
                                ),
                        ),
                )
            })
            .child(content)
            .into_any_element()
    }
    pub(super) fn remeasure_diagrams(&mut self) {
        let anchor = self.list.logical_scroll_top();
        for ix in &self.diagrams.rows {
            let row = self.visible.row(*ix);
            self.list.splice(row..row + 1, 1);
        }
        self.list.scroll_to(anchor);
    }
    fn remeasure_diagram(&mut self, ix: usize, cx: &mut Context<Self>) {
        let anchor = self.list.logical_scroll_top();
        let row = self.visible.row(ix);
        self.list.splice(row..row + 1, 1);
        self.list.scroll_to(anchor);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn sgf_preview_copy_and_source_toggle_share_the_bounded_cache(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("board.md");
        let record = "(;GM[1]SZ[9]AB[cc];B[dd];W[ee])\n";
        std::fs::write(&path, format!("```sgf move=1\n{record}```\n")).unwrap();
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|window, cx| {
            let view = MarkdownView::new(
                path,
                source,
                index,
                TextStyle {
                    font_family: "Menlo".into(),
                    font_size: px(14.),
                    line_height: 1.5,
                    tab_size: 4,
                    sticky_scroll: false,
                    max_parse_size: 0,
                },
                Arc::default(),
                cx,
            );
            view.focus.focus(window);
            view
        });
        cx.run_until_parked();
        let original = window
            .update(cx, |view, _, cx| {
                assert!(!view.parsing);
                assert!(view.diagrams.pending.is_empty());
                let cache = cx.global::<Cache>();
                assert_eq!(cache.entries.len(), 1);
                assert!(cache.bytes > 0 && cache.bytes <= BUDGET);
                let preview = cache
                    .entries
                    .values()
                    .next()
                    .unwrap()
                    .result
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    preview.caption.as_ref().map(|s| s.as_ref()),
                    Some("Game 1/1 · move 1 · main line")
                );
                preview.image.clone()
            })
            .unwrap();
        // Exercise the actual per-fence controls, including returning to a
        // raster without replacing it with a shorter loading placeholder.
        for selector in ["md-copy", "md-diagram-mode", "md-diagram-mode"] {
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            cx.run_until_parked();
            window
                .update(cx, |view, _, cx| {
                    assert!(view.diagrams.pending.is_empty());
                    let cache = cx.global::<Cache>();
                    let preview = cache
                        .entries
                        .values()
                        .next()
                        .unwrap()
                        .result
                        .as_ref()
                        .unwrap();
                    assert!(Arc::ptr_eq(&preview.image, &original));
                    assert_eq!(
                        cx.read_from_clipboard().unwrap().text().as_deref(),
                        Some(record)
                    );
                })
                .unwrap();
        }
        window
            .update(cx, |view, _, cx| {
                assert!(!view.diagrams.source.contains(&0));
                view.set_visible(false, cx);
                let cache = cx.global::<Cache>();
                assert!(cache.entries.is_empty());
                assert_eq!(cache.bytes, 0);
            })
            .unwrap();
    }

    #[test]
    fn split_sgf_fences_never_render_partial_records() {
        for source in [
            format!(
                "(;C[{}])\n",
                "x".repeat(plastext_markdown::BLOCK_TEXT_LIMIT)
            ),
            format!(
                "(;\n{})\n",
                "C[note]\n".repeat(plastext_markdown::DIAGRAM_LINE_LIMIT)
            ),
        ] {
            let document = parse(&format!("```sgf\n{source}```\n"));
            let index = DocumentIndex::initial(&document, 0);
            assert!(index.blocks.len() > 1);
            for ix in 0..index.blocks.len() {
                assert!(!complete_fence(&index, ix));
            }
            assert!(preview_rejection(&index, 0, &document.blocks[0].text, Kind::Sgf).is_some());
        }
    }
    #[test]
    fn large_tree_stays_whole_and_oversized_fences_keep_all_source() {
        let mut source = String::from("graph TD\n");
        for node in 1..384 {
            source.push_str(&format!(" {} --> {node}\n", (node - 1) / 2));
        }
        assert!(source.len() < plastext_markdown::BLOCK_TEXT_LIMIT);
        let document = parse(&format!("```mermaid\n{source}```\n"));
        let index = DocumentIndex::initial(&document, 0);
        assert_eq!(index.blocks.len(), 1);
        assert!(complete_fence(&index, 0));
        assert_eq!(document.blocks[0].text, source);

        let source = format!(
            "graph TD\n{}",
            "%% comment\n".repeat(plastext_markdown::DIAGRAM_LINE_LIMIT)
        );
        assert!(source.len() < plastext_markdown::BLOCK_TEXT_LIMIT);
        let document = parse(&format!("```mermaid\n{source}```\n"));
        let index = DocumentIndex::initial(&document, 0);
        assert_eq!(index.blocks.len(), 2);
        let reason = preview_rejection(&index, 0, &document.blocks[0].text, Kind::Mermaid).unwrap();
        assert!(reason.contains("1024 lines"));
        assert_eq!(
            document
                .blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<String>(),
            source,
            "all edges remain readable across the virtualized source blocks"
        );
    }

    #[test]
    fn only_whole_mermaid_fences_are_rendered() {
        assert_eq!(Kind::for_language("Mermaid", true), Some(Kind::Mermaid));
        assert_eq!(Kind::for_language("mermaid-js", true), None);
        assert_eq!(Kind::for_language("mermaid", false), None);
        assert_eq!(Kind::for_language("SGF", false), Some(Kind::Sgf));
        let document =
            parse("```mermaid\nflowchart LR; A-->B\n```\n\n```mermaid\nflowchart LR; C-->D\n```\n");
        let index = DocumentIndex::initial(&document, 0);
        assert!(complete_fence(&index, 0));
        assert!(complete_fence(&index, 1));
        let document = parse(&format!(
            "```mermaid\nflowchart LR\n{}\n```",
            "%% comment\n".repeat(50)
        ));
        assert_eq!(
            document.blocks.len(),
            1,
            "bounded diagrams longer than 32 lines stay whole"
        );
        let document = parse(&format!(
            "```mermaid\nflowchart LR\n{}\n```",
            "A-->B;\n".repeat(5000)
        ));
        let index = DocumentIndex::initial(&document, 0);
        assert!(index.blocks.len() > 1);
        for ix in 0..index.blocks.len() {
            assert!(!complete_fence(&index, ix));
        }
    }
}
