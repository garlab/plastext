use super::*;
use crate::scroll_sync::{Position, RenderedAnchor, Revision};
use plastext_editor::ScrollAnchor;

pub(super) struct Pending {
    position: Position,
    attempts: u8,
    applied: Option<(usize, gpui::Pixels)>,
}

impl MarkdownView {
    // A close can happen before the first measured frame. Preserve the requested
    // reading position in that case instead of replacing it with zero.
    pub(crate) fn session_scroll(&self, cx: &App) -> Option<Position> {
        self.scroll_position(cx).or_else(|| {
            self.rendered
                .then(|| self.scroll_sync.as_ref().map(|p| p.position))
                .flatten()
        })
    }
    pub(crate) fn session_offset(&self, cx: &App) -> u64 {
        self.session_scroll(cx)
            .map(|p| p.source.start)
            .or(self.pending_offset.map(|p| p as u64))
            .unwrap_or_else(|| self.position(cx))
    }

    pub(crate) fn scroll_revision(&self) -> Revision {
        if self.rendered {
            Revision::Rendered(Arc::as_ptr(&self.document) as usize)
        } else {
            Revision::Source
        }
    }

    pub(crate) fn scroll_position(&self, cx: &App) -> Option<Position> {
        if !self.rendered {
            return self
                .source_view
                .as_ref()?
                .read(cx)
                .scroll_anchor()
                .map(Into::into);
        }
        if self
            .scroll_sync
            .as_ref()
            .is_some_and(|pending| pending.applied.is_none())
            || !self.preview_complete
        {
            return None;
        }
        let top = self.list.logical_scroll_top();
        let block_index = self.visible.block(top.item_ix)?;
        let block = self.document.blocks.get(block_index)?;
        let height = self
            .list
            .bounds_for_item(top.item_ix)?
            .size
            .height
            .max(px(1.));
        let rendered = ScrollAnchor::new(
            block.source.start as u64,
            block.source.end as u64,
            top.offset_in_item / height,
        );
        Some(Position {
            source: ScrollAnchor::new(
                self.document.source_offset(block.source.start) as u64,
                self.document.source_offset(block.source.end) as u64,
                rendered.fraction,
            ),
            rendered: Some(RenderedAnchor {
                block: block_index,
                span: rendered,
            }),
        })
    }

    pub(crate) fn sync_scroll(&mut self, position: Position, cx: &mut Context<Self>) {
        if !self.rendered {
            if let Some(view) = &self.source_view {
                view.update(cx, |view, cx| view.sync_scroll(position.source, cx));
            }
            return;
        }
        self.pending_offset = None;
        self.pending_anchor = None;
        self.scroll_sync = Some(Pending {
            position,
            attempts: 0,
            applied: None,
        });
        cx.notify();
    }

    pub(crate) fn cancel_scroll_sync(&mut self, cx: &mut Context<Self>) {
        self.scroll_sync = None;
        if let Some(source) = &self.source_view {
            source.update(cx, |view, _| view.cancel_scroll_sync());
        }
    }

    pub(super) fn apply_scroll_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.rendered || !self.preview_complete || self.parsing {
            return;
        }
        let Some(mut pending) = self.scroll_sync.take() else {
            return;
        };
        let Some((block, fraction)) = self.scroll_target(pending.position) else {
            return;
        };
        self.reveal_metadata(block);
        let row = self.visible.row(block);
        let height = self.measured_scroll_row_height(row);
        let measured = height.map(|height| (row, height));
        if measured.is_none() || pending.applied != measured {
            self.list.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: height.unwrap_or_default() * fraction,
            });
        }
        pending.applied = measured;
        // Offscreen items are measured lazily. Position the block first, then
        // apply its fractional offset after layout. Bound retries for a hidden
        // or zero-sized pane; a later leader update can try again.
        if height.is_none() && pending.attempts < 3 {
            pending.attempts += 1;
            self.scroll_sync = Some(pending);
            let weak = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                weak.update(cx, |_, cx| cx.notify()).ok();
            });
        } else if height.is_some() {
            // Image, math and diagram heights can arrive later. Keep the anchor
            // while following, but only reapply it when the measured row changes.
            pending.attempts = 0;
            self.scroll_sync = Some(pending);
        }
    }

    fn scroll_target(&self, position: Position) -> Option<(usize, f32)> {
        if let Some(rendered) = position.rendered
            && let Some(entry) = self.document.blocks.get(rendered.block)
            && entry.source.start as u64 == rendered.span.start
            && entry.source.end as u64 == rendered.span.end
        {
            // Source spans may overlap or even be empty. A rendered block's
            // own fraction must not be reinterpreted as a byte in another block.
            return Some((rendered.block, rendered.span.fraction));
        }
        // Different index versions can shift block identities. Keep the source
        // mapping as a fallback, and for Source -> Rendered synchronization.
        let anchor = position
            .rendered
            .map(|rendered| rendered.span)
            .unwrap_or_else(|| {
                let source = position.source;
                ScrollAnchor::new(
                    self.document.rendered_offset(source.start as usize) as u64,
                    self.document.rendered_offset(source.end as usize) as u64,
                    source.fraction,
                )
            });
        let block = self.document.block_at(anchor.offset() as usize);
        let entry = self.document.blocks.get(block)?;
        Some((
            block,
            anchor.fraction_in(entry.source.start as u64, entry.source.end as u64),
        ))
    }

    fn measured_scroll_row_height(&self, row: usize) -> Option<gpui::Pixels> {
        let top = self.list.logical_scroll_top();
        // GPUI's bounds query hides rows before the logical scroll top, even
        // when their measurements are still cached. Query from the target row
        // and restore the position synchronously, before any layout or paint.
        // Otherwise scrolling up (or lingering in a source gap after a block)
        // paints the block's top for a frame before restoring its fraction.
        let above = row < top.item_ix;
        if above {
            self.list.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
        }
        let height = self
            .list
            .bounds_for_item(row)
            .map(|bounds| bounds.size.height);
        if above {
            self.list.scroll_to(top);
        }
        height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, size};

    #[gpui::test]
    fn rendered_scroll_distinguishes_title_author_and_date_with_the_same_source_span(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("title.tex");
        let text = format!(
            "\\title{{A long document title that wraps in a narrow pane}}\n\\author{{An author\\\\An affiliation}}\n\\date{{A draft date}}\n\\begin{{document}}\n\\maketitle\n{}\\end{{document}}\n",
            "A paragraph of body text.\n\n".repeat(100),
        );
        std::fs::write(&path, &text).unwrap();
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        cx.update(crate::init);
        let windows: Vec<_> = [360., 700.]
            .into_iter()
            .map(|width| {
                let window = cx.add_window(|_, cx| {
                    MarkdownView::new_latex(
                        path.clone(),
                        source.clone(),
                        index.clone(),
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
                    )
                });
                VisualTestContext::from_window(window.into(), cx)
                    .simulate_resize(size(px(width), px(600.)));
                window
            })
            .collect();
        cx.run_until_parked();
        windows[0]
            .update(cx, |view, _, _| {
                let blocks = &view.document.blocks;
                assert_eq!(blocks[0].source, blocks[1].source);
                assert_eq!(blocks[0].source, blocks[2].source);
                assert_eq!(blocks[0].source.start, text.find("\\maketitle").unwrap());
            })
            .unwrap();
        let mut previous: Option<Position> = None;
        for (step, (row, fraction)) in [
            (0, 0.),
            (1, 0.),
            (2, 0.),
            (1, 0.3),
            (2, 0.4),
            (0, 0.5),
            (0, 0.),
        ]
        .into_iter()
        .enumerate()
        {
            let leader = windows[step % 2];
            let follower = windows[(step + 1) % 2];
            leader
                .update(cx, |view, _, cx| {
                    view.cancel_scroll_sync(cx);
                    let height = view.measured_scroll_row_height(row).unwrap();
                    view.list.scroll_to(ListOffset {
                        item_ix: row,
                        offset_in_item: height * fraction,
                    });
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            let anchor = leader
                .update(cx, |view, _, cx| view.scroll_position(cx).unwrap())
                .unwrap();
            if step == 1 || step == 2 {
                let previous = previous.unwrap();
                assert_eq!(previous.source, anchor.source);
                assert_ne!(
                    previous, anchor,
                    "the workspace must detect a change between blocks sharing a source position"
                );
            }
            previous = Some(anchor);
            follower
                .update(cx, |view, _, cx| view.sync_scroll(anchor, cx))
                .unwrap();
            for _ in 0..4 {
                // GPUI's test platform does not schedule display-link callbacks.
                follower.update(cx, |_, _, cx| cx.notify()).unwrap();
                cx.run_until_parked();
            }
            follower
                .update(cx, |view, _, _| {
                    let top = view.list.logical_scroll_top();
                    assert_eq!(
                        top.item_ix, row,
                        "rendered block identity was lost at step {step}"
                    );
                    let height = view.measured_scroll_row_height(row).unwrap();
                    assert!((top.offset_in_item / height - fraction).abs() < 0.001);
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn source_scroll_reuses_heights_above_the_viewport_without_a_correction_frame(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.tex");
        std::fs::write(&path, "A paragraph with enough words to wrap across several lines in the rendered pane.\n\n% Source-only comment\n\n".repeat(80)).unwrap();
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|_, cx| {
            MarkdownView::new_latex(
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
            )
        });
        VisualTestContext::from_window(window.into(), cx).simulate_resize(size(px(360.), px(400.)));
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                view.list.scroll_to(ListOffset {
                    item_ix: 20,
                    offset_in_item: px(0.),
                });
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let (source, height) = window
            .update(cx, |view, _, _| {
                let entry = &view.document.blocks[view.visible.block(20).unwrap()];
                let height = view.list.bounds_for_item(20).unwrap().size.height;
                (entry.source.clone(), height)
            })
            .unwrap();
        assert!(height > px(20.));
        // All of these positions are already measured. Source comments and
        // blank lines keep pointing at the end of the preceding rendered block.
        // No input should flash that block's start, including repeated updates
        // at its end and a direction reversal into the previous block.
        for fraction in [1., 1., 0.98, 0.95, 0.9, 0.96, 1.] {
            window
                .update(cx, |view, window, cx| {
                    view.list.scroll_to(ListOffset {
                        item_ix: 21,
                        offset_in_item: px(0.),
                    });
                    assert!(view.list.bounds_for_item(20).is_none());
                    let at = if fraction == 1. {
                        ScrollAnchor::new(source.end as u64 + 1, source.end as u64 + 2, 0.5)
                    } else {
                        ScrollAnchor::new(source.start as u64, source.end as u64, fraction)
                    };
                    view.sync_scroll(at.into(), cx);
                    view.apply_scroll_sync(window, cx);
                    let top = view.list.logical_scroll_top();
                    assert_eq!(top.item_ix, 20);
                    assert!(
                        (top.offset_in_item - height * fraction).abs() < px(0.01),
                        "fraction {fraction}: {top:?}"
                    );
                    assert_eq!(
                        view.scroll_sync.as_ref().unwrap().applied,
                        Some((20, height)),
                        "must be positioned before the first paint"
                    );
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn scroll_sync_keeps_expanded_include_positions_between_rendered_panes(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.tex");
        let root = "\\section{Before}\n\\input{part}\n\\section{After}\n";
        std::fs::write(&path, root).unwrap();
        std::fs::write(
            dir.path().join("part.tex"),
            "A paragraph inside an included file, with enough words to wrap.\n\n".repeat(400),
        )
        .unwrap();
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        cx.update(crate::init);
        let windows: Vec<_> = [500., 800.]
            .into_iter()
            .map(|width| {
                let window = cx.add_window(|_, cx| {
                    MarkdownView::new_latex(
                        path.clone(),
                        source.clone(),
                        index.clone(),
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
                    )
                });
                VisualTestContext::from_window(window.into(), cx)
                    .simulate_resize(size(px(width), px(600.)));
                window
            })
            .collect();
        cx.run_until_parked();
        windows[0]
            .update(cx, |view, _, cx| {
                view.list.scroll_to(ListOffset {
                    item_ix: view.visible.row(200),
                    offset_in_item: px(17.),
                });
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let anchor = windows[0]
            .update(cx, |view, _, cx| view.scroll_position(cx).unwrap())
            .unwrap();
        assert!(anchor.rendered.unwrap().span.start > root.len() as u64);
        assert!(
            anchor.source.start
                <= root.find("\\input").unwrap() as u64 + "\\input{part}".len() as u64
        );
        windows[1]
            .update(cx, |view, _, cx| view.sync_scroll(anchor, cx))
            .unwrap();
        for _ in 0..4 {
            // The test platform does not run display-link callbacks.
            windows[1].update(cx, |_, _, cx| cx.notify()).unwrap();
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
        let actual = windows[1]
            .update(cx, |view, _, cx| view.scroll_position(cx).unwrap())
            .unwrap();
        assert_eq!(
            actual.rendered.unwrap().span.start,
            anchor.rendered.unwrap().span.start
        );
        assert!(
            (actual.rendered.unwrap().span.fraction - anchor.rendered.unwrap().span.fraction).abs()
                < 0.001
        );
    }
}
