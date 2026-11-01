use super::{
    CsvView, HEIGHT,
    columns::{ColumnDrag, display_chars},
    copy,
    selection::{self, Drag, Position, Selection, Unit},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, HighlightStyle, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    StyledText, TextLayout, Window, actions, canvas, div, point, prelude::*, px,
};
use plastext_editor::Theme;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

actions!(csv_view, [Copy, SelectAll, ClearSelection]);

pub(crate) fn init(cx: &mut App) {
    let context = Some("CsvTable");
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("escape", ClearSelection, context),
    ]);
}

#[derive(Clone)]
pub(super) struct CellHit {
    row: u64,
    column: usize,
    source: SharedString,
    layout: TextLayout,
    bounds: Bounds<Pixels>,
}

pub(super) type FrameHits = Rc<RefCell<Vec<CellHit>>>;

impl CellHit {
    fn position(&self, pointer: Point<Pixels>) -> Position {
        let index = self
            .layout
            .index_for_position(point(pointer.x, self.layout.bounds().center().y))
            .unwrap_or_else(|i| i);
        Position {
            row: self.row,
            column: self.column,
            byte: selection::source_offset(&self.source, index),
        }
    }
}

impl CsvView {
    pub(super) fn selection_changed(&mut self, cx: &mut Context<Self>) {
        self.copy_cancel.store(true, Ordering::Relaxed);
        self._copy = None;
        cx.notify();
    }

    pub(super) fn clear_selection(
        &mut self,
        _: &ClearSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = Selection::None;
        self.selection_drag = None;
        self.selection_changed(cx);
    }

    pub(super) fn select_all(
        &mut self,
        _: &SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window);
        self.selection = Selection::All;
        self.selection_drag = None;
        self.selection_changed(cx);
    }

    pub(super) fn select_row(
        &mut self,
        number: u64,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window);
        if number == 0 {
            self.select_all(&SelectAll, window, cx);
        } else {
            if self.selection == Selection::All {
                self.selection = Selection::Rows {
                    anchor: 0,
                    ranges: self.selection.rows(self.index.progress().rows),
                };
            }
            self.selection
                .select_row(number, event.modifiers.shift, event.modifiers.platform);
            self.selection_drag = (!event.modifiers.platform).then_some(Drag::Rows);
            self.selection_changed(cx);
        }
        cx.stop_propagation();
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let progress = self.index.progress();
        if self.selection.rows(progress.rows).is_empty() {
            return;
        }
        if self.selection == Selection::All && !progress.complete {
            self.notice = Some(
                "The CSV is still indexing. Try copying all rows once indexing finishes.".into(),
            );
            cx.notify();
            return;
        }
        self.copy_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.copy_cancel = cancel.clone();
        let document_cancel = self.cancel.clone();
        let selection = self.selection.clone();
        let source = self.source.clone();
        let index = self.index.clone();
        let delimiter = self.delimiter;
        let copied = cx.background_executor().spawn(async move {
            copy::selected_text(
                &selection,
                progress.rows,
                |first, count| index.page(&source, delimiter, first, count),
                || cancel.load(Ordering::Relaxed) || document_cancel.load(Ordering::Relaxed),
                copy::MAX_COPY,
            )
        });
        self._copy = Some(cx.spawn(async move |this, cx| {
            let result = copied.await;
            this.update(cx, |this, cx| {
                this._copy = None;
                match result {
                    Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                    Err(error) => this.notice = Some(format!("Couldn't copy: {error}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_cell(
        &self,
        row: u64,
        column: usize,
        source: String,
        selectable: bool,
        theme: Theme,
        hits: FrameHits,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selection = self.selection.cell_range(row, column, source.len());
        let displayed = display_chars(&source).collect::<String>();
        let text = StyledText::new(displayed).with_highlights(
            selection.filter(|r| !r.is_empty()).map(|range| {
                (
                    selection::display_offset(&source, range.start)
                        ..selection::display_offset(&source, range.end),
                    HighlightStyle {
                        background_color: Some(theme.selection.into()),
                        ..Default::default()
                    },
                )
            }),
        );
        let hit = CellHit {
            row,
            column,
            source: source.into(),
            layout: text.layout().clone(),
            bounds: Bounds::default(),
        };
        let painted = hit.clone();
        div()
            .id(("csv-cell", column))
            .relative()
            .w(px(self.columns.width(column) * self.zoom))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .px(px(8. * self.zoom))
            .border_l_1()
            .border_color(theme.border)
            .when(selectable, |cell| {
                cell.cursor(CursorStyle::IBeam).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.focus.focus(window);
                        let unit = match event.click_count {
                            1 => Unit::Character,
                            2 => Unit::Word,
                            _ => Unit::Cell,
                        };
                        let target =
                            selection::unit_range(&hit.source, hit.position(event.position), unit);
                        let anchor = match this.selection {
                            Selection::Text { anchor, .. } if event.modifiers.shift => {
                                anchor..anchor
                            }
                            _ => target.clone(),
                        };
                        this.selection.extend_text(anchor.clone(), target);
                        this.selection_drag = Some(Drag::Text { anchor, unit });
                        this.selection_changed(cx);
                        cx.stop_propagation();
                    }),
                )
            })
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(text),
            )
            .when(selectable, |cell| {
                cell.child(
                    canvas(
                        |bounds, _, _| bounds,
                        move |_, bounds, window, _| {
                            let bounds = bounds.intersect(&window.content_mask().bounds);
                            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                                hits.borrow_mut().push(CellHit {
                                    bounds,
                                    ..painted.clone()
                                });
                            }
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
            })
            .when(row == 0, |cell| {
                cell.child(
                    div()
                        .id(("csv-column-divider", column))
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(px(7.))
                        .h_full()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .border_r_1()
                        .border_color(theme.border)
                        .hover(|style| style.bg(theme.border))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                this.focus.focus(window);
                                this.selection_drag = None;
                                if event.click_count == 2 {
                                    this.columns.reset(column);
                                    this.column_drag = None;
                                } else {
                                    this.column_drag = Some(ColumnDrag {
                                        column,
                                        start_x: event.position.x.into(),
                                        start_width: this.columns.width(column),
                                    });
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
            })
    }

    pub(super) fn selection_layer(&self, hits: FrameHits, cx: &Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        canvas(
            |bounds, _, _| bounds,
            move |_, bounds, window, cx| {
                let moved = weak.clone();
                let frame_hits = hits.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    moved
                        .update(cx, |this, cx| {
                            if event.dragging() {
                                this.drag_selection(
                                    event.position,
                                    bounds,
                                    &frame_hits.borrow(),
                                    cx,
                                );
                            } else {
                                this.selection_drag = None;
                            }
                        })
                        .ok();
                });
                let released = weak.clone();
                let frame_hits = hits.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        released
                            .update(cx, |this, cx| {
                                this.drag_selection(
                                    event.position,
                                    bounds,
                                    &frame_hits.borrow(),
                                    cx,
                                );
                                this.selection_drag = None;
                            })
                            .ok();
                    }
                });
                // Keep extending while the pointer is held beyond an edge,
                // including when it stops moving between mouse events.
                let pointer = window.mouse_position();
                let auto_scroll = weak.upgrade().is_some_and(|view| {
                    let view = view.read(cx);
                    view.selection_drag.is_some()
                        && (pointer.y < bounds.top() + px(HEIGHT * view.zoom)
                            || pointer.y > bounds.bottom()
                            || pointer.x < bounds.left()
                            || pointer.x > bounds.right())
                });
                if auto_scroll {
                    let next = weak.clone();
                    let frame_hits = hits.clone();
                    window.on_next_frame(move |window, cx| {
                        next.update(cx, |this, cx| {
                            if window.is_window_active() {
                                this.drag_selection(
                                    window.mouse_position(),
                                    bounds,
                                    &frame_hits.borrow(),
                                    cx,
                                );
                            } else {
                                this.selection_drag = None;
                            }
                        })
                        .ok();
                    });
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn drag_selection(
        &mut self,
        pointer: Point<Pixels>,
        bounds: Bounds<Pixels>,
        hits: &[CellHit],
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.selection_drag.clone() else {
            return;
        };
        let header_drag = matches!(&drag, Drag::Text { anchor, .. } if anchor.start.row == 0)
            && pointer.y < bounds.top() + px(HEIGHT * self.zoom);
        let gap = |value: Pixels, start: Pixels, end: Pixels| -> f32 {
            f32::from((start - value).max(value - end).max(px(0.)))
        };
        let nearest = hits
            .iter()
            .filter(|hit| {
                hit.row != 0
                    || (!matches!(drag, Drag::Rows) && (self.first_row == 0 || header_drag))
            })
            .min_by(|a, b| {
                gap(pointer.y, a.bounds.top(), a.bounds.bottom())
                    .total_cmp(&gap(pointer.y, b.bounds.top(), b.bounds.bottom()))
                    .then_with(|| {
                        gap(pointer.x, a.bounds.left(), a.bounds.right()).total_cmp(&gap(
                            pointer.x,
                            b.bounds.left(),
                            b.bounds.right(),
                        ))
                    })
            });
        let Some(hit) = nearest else { return };
        let old = self.selection.clone();
        match drag {
            Drag::Rows => self.selection.select_row(hit.row, true, false),
            Drag::Text { anchor, unit } => self.selection.extend_text(
                anchor,
                selection::unit_range(&hit.source, hit.position(pointer), unit),
            ),
        }
        if self.selection != old {
            self.selection_changed(cx);
        }
        if !header_drag {
            let vertical = self.scroll.0.borrow().base_handle.clone();
            let mut offset = vertical.offset();
            let old = offset;
            offset.y = edge_offset(
                pointer.y,
                bounds.top() + px(HEIGHT * self.zoom),
                bounds.bottom(),
                offset.y,
                vertical.max_offset().height,
                px(HEIGHT * self.zoom),
            );
            if offset != old {
                vertical.set_offset(offset);
                cx.notify();
            }
        }
        if matches!(self.selection_drag, Some(Drag::Text { .. })) {
            let mut offset = self.horizontal_scroll.offset();
            let old = offset;
            offset.x = edge_offset(
                pointer.x,
                bounds.left(),
                bounds.right(),
                offset.x,
                self.horizontal_scroll.max_offset().width,
                px(24.),
            );
            if offset != old {
                self.horizontal_scroll.set_offset(offset);
                cx.notify();
            }
        }
    }
}

fn edge_offset(
    pointer: Pixels,
    start: Pixels,
    end: Pixels,
    offset: Pixels,
    max: Pixels,
    step: Pixels,
) -> Pixels {
    let delta = if pointer < start {
        step
    } else if pointer > end {
        -step
    } else {
        px(0.)
    };
    (offset + delta).clamp(-max, px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_past_edges_scrolls_and_reveals_partial_first_and_last_rows() {
        let offset = |pointer, scroll| {
            edge_offset(
                px(pointer),
                px(100.),
                px(600.),
                px(scroll),
                px(1000.),
                px(28.),
            )
        };
        assert_eq!(offset(90., -10.), px(0.));
        assert_eq!(offset(610., -990.), px(-1000.));
        assert_eq!(offset(300., -400.), px(-400.));
        assert_eq!(offset(90., -400.), px(-372.));
        assert_eq!(offset(610., -400.), px(-428.));
    }
}
