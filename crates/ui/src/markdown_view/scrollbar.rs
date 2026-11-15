//! Scroll in logical block coordinates: GPUI's pixel totals exclude unmeasured
//! rows. Measuring the whole document for a scrollbar would defeat virtualization.
use super::*;
use crate::scroll_sync::{Position, RenderedAnchor};
use gpui::{
    Bounds, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    canvas, fill, point, size,
};
use plastext_editor::ScrollAnchor;

const WIDTH: f32 = 12.;
const MIN_THUMB: f32 = 24.;

#[derive(Default)]
pub(super) struct State {
    layout: Option<Layout>,
    pub drag: Option<Drag>,
}

#[derive(Clone, Copy)]
struct Layout {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
    max_top: f64,
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    layout: Layout,
    grab: Pixels,
    fraction: f32,
    revision: usize,
}

impl Layout {
    fn for_list(list: &ListState, track: Bounds<Pixels>) -> Option<Self> {
        let count = list.item_count();
        if count == 0 || track.size.height <= px(0.) {
            return None;
        }
        let top = list.logical_scroll_top();
        let first = list.bounds_for_item(top.item_ix)?;
        let start = top.item_ix as f64
            + (top.offset_in_item / first.size.height.max(px(1.))).clamp(0., 1.) as f64;
        let bottom = list.viewport_bounds().bottom();
        let mut end = start;
        // Only inspect rows already laid out in this viewport, never parse or
        // measure offscreen content. Fractions also cover a single very tall row.
        for row in top.item_ix..count {
            let Some(bounds) = list.bounds_for_item(row) else {
                break;
            };
            end = row as f64
                + ((bottom - bounds.top()) / bounds.size.height.max(px(1.))).clamp(0., 1.) as f64;
            if bounds.bottom() >= bottom {
                break;
            }
        }
        let visible = (end - start).max(0.);
        let max_top = count as f64 - visible;
        if max_top <= f64::EPSILON {
            return None;
        }
        let height = (track.size.height * (visible / count as f64) as f32)
            .max(px(MIN_THUMB))
            .min(track.size.height);
        let fraction = (start / max_top).clamp(0., 1.) as f32;
        Some(Self {
            track,
            thumb: Bounds::new(
                point(
                    track.left() + px(3.),
                    track.top() + (track.size.height - height) * fraction,
                ),
                size(px(WIDTH - 6.), height),
            ),
            max_top,
        })
    }
}

impl MarkdownView {
    fn drag_scrollbar(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.scrollbar.drag else {
            return;
        };
        let travel = drag.layout.track.size.height - drag.layout.thumb.size.height;
        if travel <= px(0.) {
            return;
        }
        drag.fraction = ((y - drag.grab - drag.layout.track.top()) / travel).clamp(0., 1.);
        let fraction = drag.fraction;
        let target = fraction as f64 * drag.layout.max_top;
        self.scroll_sync = None;
        if fraction == 0. || fraction == 1. {
            // List layout fills backwards from the end, even when the last rows
            // have never been visited or have very different heights.
            self.list.scroll_to(ListOffset {
                item_ix: if fraction == 0. {
                    0
                } else {
                    self.visible.len()
                },
                offset_in_item: px(0.),
            });
            cx.notify();
            return;
        }
        let row = (target.floor() as usize).min(self.visible.len().saturating_sub(1));
        let Some(block) = self.visible.block(row) else {
            return;
        };
        let entry = &self.document.blocks[block];
        let span = ScrollAnchor::new(
            entry.source.start as u64,
            entry.source.end as u64,
            target.fract() as f32,
        );
        // Reuse the bounded fractional-position retry for an unmeasured row.
        // This also keeps a newly loaded image/diagram from losing the target.
        self.sync_scroll(
            Position {
                source: ScrollAnchor::new(
                    self.document.source_offset(entry.source.start) as u64,
                    self.document.source_offset(entry.source.end) as u64,
                    span.fraction,
                ),
                rendered: Some(RenderedAnchor { block, span }),
            },
            cx,
        );
    }

    pub(super) fn scrollbar_layer(&self, cx: &Context<Self>) -> impl IntoElement {
        let measured = cx.entity().downgrade();
        let painted = measured.clone();
        div()
            .id("md-scrollbar")
            .debug_selector(|| "md-scrollbar".into())
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(WIDTH))
            // Keep the pane's capture handler and the list's wheel handler
            // reachable. Only a scrollbar press/drag consumes pointer events.
            .cursor(gpui::CursorStyle::Arrow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let Some(layout) = this.scrollbar.layout else {
                        return;
                    };
                    this.focus.focus(window);
                    this.reading.stop();
                    this.scroll_sync = None;
                    let on_thumb = layout.thumb.contains(&event.position);
                    let travel = layout.track.size.height - layout.thumb.size.height;
                    this.scrollbar.drag = Some(Drag {
                        layout,
                        grab: if on_thumb {
                            event.position.y - layout.thumb.top()
                        } else {
                            layout.thumb.size.height / 2.
                        },
                        fraction: if travel > px(0.) {
                            (layout.thumb.top() - layout.track.top()) / travel
                        } else {
                            0.
                        },
                        revision: Arc::as_ptr(&this.document) as usize,
                    });
                    if !on_thumb {
                        this.drag_scrollbar(event.position.y, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        measured
                            .update(cx, |this, _| {
                                if this.scrollbar.drag.is_some_and(|drag| {
                                    drag.layout.track != bounds
                                        || drag.revision != Arc::as_ptr(&this.document) as usize
                                }) {
                                    this.scrollbar.drag = None;
                                }
                                this.scrollbar.layout = Layout::for_list(&this.list, bounds);
                            })
                            .ok();
                    },
                    move |_, _, window, cx| {
                        if let Some(view) = painted.upgrade() {
                            let state = &view.read(cx).scrollbar;
                            let thumb = if let Some(drag) = state.drag {
                                let mut thumb = drag.layout.thumb;
                                // Freeze the extent during a drag; measuring newly
                                // visible blocks must not move the thumb from the pointer.
                                thumb.origin.y = drag.layout.track.top()
                                    + (drag.layout.track.size.height - thumb.size.height)
                                        * drag.fraction;
                                Some(thumb)
                            } else {
                                state.layout.map(|layout| layout.thumb)
                            };
                            if let Some(thumb) = thumb {
                                window.paint_quad(
                                    fill(thumb, Theme::get(window, cx).scrollbar_thumb)
                                        .corner_radii(px(3.)),
                                );
                            }
                        }
                        let moved = painted.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            moved
                                .update(cx, |this, cx| {
                                    if this.scrollbar.drag.is_some() {
                                        if event.dragging() {
                                            this.drag_scrollbar(event.position.y, cx);
                                            cx.stop_propagation();
                                        } else {
                                            this.scrollbar.drag = None;
                                            cx.notify();
                                        }
                                    }
                                })
                                .ok();
                        });
                        let released = painted.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                released
                                    .update(cx, |this, cx| {
                                        if this.scrollbar.drag.take().is_some() {
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, WindowHandle};
    use plastext_markdown::selection::Selection;

    fn open(
        cx: &mut TestAppContext,
        path: PathBuf,
        format: DocumentFormat,
    ) -> WindowHandle<MarkdownView> {
        let (source, index) = plastext_buffer::open(&path, Default::default()).unwrap();
        cx.update(crate::init);
        let window = cx.add_window(|_, cx| {
            MarkdownView::new_format(
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
                format,
                cx,
            )
        });
        VisualTestContext::from_window(window.into(), cx).simulate_resize(size(px(640.), px(480.)));
        settle(cx);
        window
    }

    fn settle(cx: &mut TestAppContext) {
        cx.run_until_parked();
        // The test platform does not schedule display ticks for newly measured rows.
        for _ in 0..4 {
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
    }

    fn layout(cx: &mut TestAppContext, window: WindowHandle<MarkdownView>) -> Layout {
        window
            .update(cx, |view, _, _| {
                view.scrollbar.layout.expect("visible scrollbar")
            })
            .unwrap()
    }

    #[gpui::test]
    fn scrollbar_reaches_unmeasured_rows_and_both_ends_without_selecting_text(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let text: String = (0..500)
            .map(|i| {
                format!(
                    "Paragraph {i}. {}\n\n",
                    "Some readable text. ".repeat(1 + i % 12)
                )
            })
            .collect();
        for format in [DocumentFormat::Markdown, DocumentFormat::Latex] {
            let path = dir.path().join(if format == DocumentFormat::Latex {
                "main.tex"
            } else {
                "main.md"
            });
            std::fs::write(&path, &text).unwrap();
            let window = open(cx, path, format);
            let before = layout(cx, window);
            assert_eq!(before.thumb.top(), before.track.top());
            let count = window
                .update(cx, |view, _, _| {
                    view.reading.selected = Selection::All;
                    let count = view.list.item_count();
                    assert!(count >= 500);
                    assert!(
                        view.list.bounds_for_item(count - 1).is_none(),
                        "offscreen rows stay unmeasured"
                    );
                    count
                })
                .unwrap();
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            visual.simulate_mouse_down(
                before.thumb.center(),
                MouseButton::Left,
                Default::default(),
            );
            // A drag keeps working after the pointer leaves the narrow scrollbar.
            let middle = point(before.track.left() - px(100.), before.track.center().y);
            visual.simulate_mouse_move(middle, MouseButton::Left, Default::default());
            settle(cx);
            window
                .update(cx, |view, _, _| {
                    let row = view.list.logical_scroll_top().item_ix;
                    assert!(
                        (count * 4 / 10..count * 6 / 10).contains(&row),
                        "middle row: {row}/{count}"
                    );
                    assert!(matches!(view.reading.selected, Selection::All));
                    assert_eq!(
                        view.scrollbar.drag.unwrap().layout.thumb.size,
                        before.thumb.size
                    );
                })
                .unwrap();

            let end = point(middle.x, before.track.bottom() + px(100.));
            VisualTestContext::from_window(window.into(), cx).simulate_mouse_move(
                end,
                MouseButton::Left,
                Default::default(),
            );
            settle(cx);
            window
                .update(cx, |view, _, _| {
                    let last = view.list.bounds_for_item(count - 1).unwrap();
                    assert!((last.bottom() - view.list.viewport_bounds().bottom()).abs() < px(1.));
                })
                .unwrap();
            let top = point(middle.x, before.track.top() - px(100.));
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            visual.simulate_mouse_move(top, MouseButton::Left, Default::default());
            visual.simulate_mouse_up(top, MouseButton::Left, Default::default());
            settle(cx);
            window
                .update(cx, |view, _, _| {
                    assert_eq!(view.list.logical_scroll_top().item_ix, 0);
                    assert_eq!(view.list.logical_scroll_top().offset_in_item, px(0.));
                    assert!(view.scrollbar.drag.is_none());
                    assert!(matches!(view.reading.selected, Selection::All));
                })
                .unwrap();

            // Wheel input works over the document and over the scrollbar itself.
            for pointer in [middle, before.track.center()] {
                let old_top = layout(cx, window).thumb.top();
                VisualTestContext::from_window(window.into(), cx).simulate_event(
                    ScrollWheelEvent {
                        position: pointer,
                        delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                        ..Default::default()
                    },
                );
                settle(cx);
                assert!(layout(cx, window).thumb.top() > old_top);
            }
            let track = layout(cx, window).track;
            VisualTestContext::from_window(window.into(), cx)
                .simulate_click(track.center(), Default::default());
            settle(cx);
            assert!(
                window
                    .update(cx, |view, _, _| view.list.logical_scroll_top().item_ix
                        > count / 3)
                    .unwrap()
            );
            // Outline/link navigation also supersedes the retained jump.
            let target = text.find("Paragraph 40.").unwrap();
            window
                .update(cx, |view, _, cx| view.scroll_to_offset(target as u64, cx))
                .unwrap();
            settle(cx);
            window
                .update(cx, |view, _, cx| {
                    assert_eq!(view.position(cx), target as u64)
                })
                .unwrap();
            // Keyboard navigation must cancel the retained fractional jump.
            VisualTestContext::from_window(window.into(), cx)
                .simulate_click(track.center(), Default::default());
            settle(cx);
            cx.simulate_keystrokes(window.into(), "g");
            settle(cx);
            assert_eq!(layout(cx, window).thumb.top(), before.track.top());
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    #[gpui::test]
    fn scrollbar_handles_one_tall_block_and_disappears_when_content_fits(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "A short paragraph.".to_string(),
            "A long paragraph with wrapped text. ".repeat(300),
        ] {
            let path = dir.path().join("main.md");
            std::fs::write(&path, &text).unwrap();
            let window = open(cx, path, DocumentFormat::Markdown);
            if text.len() < 100 {
                window
                    .update(cx, |view, _, _| assert!(view.scrollbar.layout.is_none()))
                    .unwrap();
            } else {
                let bar = layout(cx, window);
                VisualTestContext::from_window(window.into(), cx)
                    .simulate_click(bar.track.center(), Default::default());
                settle(cx);
                window
                    .update(cx, |view, _, _| {
                        assert_eq!(view.list.item_count(), 1);
                        assert_eq!(view.list.logical_scroll_top().item_ix, 0);
                        assert!(view.list.logical_scroll_top().offset_in_item > px(100.));
                    })
                    .unwrap();
                window
                    .update(cx, |view, _, cx| view.set_zoom(0.1, px(1.4), cx))
                    .unwrap();
                VisualTestContext::from_window(window.into(), cx)
                    .simulate_resize(size(px(1600.), px(1800.)));
                settle(cx);
                window
                    .update(cx, |view, _, _| assert!(view.scrollbar.layout.is_none()))
                    .unwrap();
            }
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
