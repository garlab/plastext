use super::*;
use gpui::{
    Bounds, CursorStyle, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, TextLayout, canvas,
};
use plastext_markdown::selection::{self as model, Position, Selection, Unit};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

actions!(markdown_selection, [Copy, SelectAll, Clear]);
pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, Some("MarkdownPreview")),
        KeyBinding::new("cmd-a", SelectAll, Some("MarkdownPreview")),
        KeyBinding::new("escape", Clear, Some("MarkdownPreview")),
    ]);
}

#[derive(Default)]
pub(super) struct State {
    pub selected: Selection,
    pub suppress_link: bool,
    drag: Option<(Range<Position>, Unit, Point<Pixels>)>,
    pub hits: Rc<RefCell<Vec<Hit>>>,
    copy_cancel: Arc<AtomicBool>,
    copying: Option<Task<()>>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.copy_cancel.store(true, Ordering::Relaxed);
    }
}
impl State {
    pub fn stop(&mut self) {
        self.drag = None;
        self.copy_cancel.store(true, Ordering::Relaxed);
        self.copying = None;
    }
}

#[derive(Clone, Debug)]
pub(super) struct Replacement {
    pub source: Range<usize>,
    pub display: Range<usize>,
}
/// Mapping between TeX bytes and the single visual object replacing them.
fn source_offset(display: usize, replacements: &[Replacement]) -> usize {
    let mut source_end = 0;
    let mut display_end = 0;
    for map in replacements {
        if display < map.display.start {
            break;
        }
        if display < map.display.end {
            return if (display - map.display.start) * 2 < map.display.len() {
                map.source.start
            } else {
                map.source.end
            };
        }
        source_end = map.source.end;
        display_end = map.display.end;
    }
    source_end + display.saturating_sub(display_end)
}
fn display_offset(source: usize, end: bool, replacements: &[Replacement]) -> usize {
    let mut source_end = 0;
    let mut display_end = 0;
    for map in replacements {
        if source <= map.source.start {
            break;
        }
        if source < map.source.end {
            return if end {
                map.display.end
            } else {
                map.display.start
            };
        }
        source_end = map.source.end;
        display_end = map.display.end;
    }
    display_end + source.saturating_sub(source_end)
}

pub(super) fn display_range(source: Range<usize>, replacements: &[Replacement]) -> Range<usize> {
    display_offset(source.start, false, replacements)
        ..display_offset(source.end, true, replacements)
}

/// GPUI's TextLayout hit test does not include its paint-time text alignment.
/// Match the width of the wrapped row, including center/right-aligned tables.
fn index_at(layout: &TextLayout, mut pointer: Point<Pixels>, align: gpui::TextAlign) -> usize {
    if align != gpui::TextAlign::Left {
        let mut at = 0;
        while let Some(line) = layout.line_layout_for_index(at) {
            let top = layout.position_for_index(at).unwrap().y;
            let height = layout.line_height();
            if pointer.y < top + line.size(height).height || at + line.len() >= layout.len() {
                let row = (f32::from((pointer.y - top).max(px(0.))) / f32::from(height)) as usize;
                let glyph_x = |boundary: &gpui::WrapBoundary| {
                    line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
                        .position
                        .x
                };
                let row = row.min(line.wrap_boundaries.len());
                let start = row
                    .checked_sub(1)
                    .map_or(px(0.), |r| glyph_x(&line.wrap_boundaries[r]));
                let end = line
                    .wrap_boundaries
                    .get(row)
                    .map_or(line.unwrapped_layout.width, glyph_x);
                let spare = layout.bounds().size.width - (end - start);
                pointer.x -= if align == gpui::TextAlign::Center {
                    spare / 2.
                } else {
                    spare
                };
                break;
            }
            at += line.len() + 1;
        }
    }
    layout.index_for_position(pointer).unwrap_or_else(|i| i)
}

#[derive(Clone)]
pub(super) struct Hit {
    block: usize,
    offset: usize,
    text: SharedString,
    replacements: Rc<Vec<Replacement>>,
    layout: Option<TextLayout>,
    alignment: Rc<Cell<gpui::TextAlign>>,
    bounds: Bounds<Pixels>,
}
impl Hit {
    fn target(&self, pointer: Point<Pixels>, unit: Unit) -> Range<Position> {
        let range = if let Some(layout) = &self.layout {
            let at = index_at(layout, pointer, self.alignment.get());
            if let Some(map) = self.replacements.iter().find(|r| r.display.contains(&at)) {
                match unit {
                    Unit::Character => {
                        let at = source_offset(at, &self.replacements);
                        at..at
                    }
                    Unit::Word => map.source.clone(),
                    Unit::Block => 0..self.text.len(),
                }
            } else {
                model::unit_range(&self.text, source_offset(at, &self.replacements), unit)
            }
        } else {
            0..self.text.len()
        };
        Position {
            block: self.block,
            byte: self.offset + range.start,
        }..Position {
            block: self.block,
            byte: self.offset + range.end,
        }
    }
}

impl MarkdownView {
    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        self.reading.copy_cancel.store(true, Ordering::Relaxed);
        self.reading.copying = None;
        cx.notify();
    }
    pub(super) fn clear_selection(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.reading.selected = Selection::None;
        self.reading.stop();
        cx.notify();
    }
    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.reading.selected = Selection::All;
        self.reading.stop();
        cx.notify();
    }
    pub(super) fn copy_selection(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.reading.selected.range().is_some_and(|r| !r.is_empty()) {
            return;
        }
        if !self.preview_complete {
            self.notice =
                Some("Markdown is still indexing. Try copying once indexing finishes.".into());
            cx.notify();
            return;
        }
        self.reading.copy_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.reading.copy_cancel = cancel.clone();
        let selection = self.reading.selected.clone();
        let source = self.preview_source.clone();
        let document = self.document.clone();
        let expanded = self.expanded.clone();
        let work_cancel = cancel.clone();
        let copied = cx.background_executor().spawn(async move {
            model::copy_selection(
                &document,
                &source,
                &selection,
                &expanded,
                || work_cancel.load(Ordering::Relaxed),
                64_000_000,
            )
        });
        self.reading.copying = Some(cx.spawn(async move |this, cx| {
            let result = copied.await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            this.update(cx, |this, cx| {
                if !Arc::ptr_eq(&cancel, &this.reading.copy_cancel) {
                    return;
                }
                this.reading.copying = None;
                match result {
                    Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                    Err(error) => this.notice = Some(format!("Couldn't copy: {error}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn selected_range(
        &self,
        block: usize,
        range: Range<usize>,
        maps: &[Replacement],
    ) -> Option<Range<usize>> {
        let selected = self.reading.selected.block_range(block, range.end)?;
        let start = selected.start.max(range.start);
        let end = selected.end.min(range.end);
        (start < end).then(|| {
            display_offset(start - range.start, false, maps)
                ..display_offset(end - range.start, true, maps)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn selectable(
        &self,
        block: usize,
        offset: usize,
        text: SharedString,
        layout: Option<TextLayout>,
        replacements: Vec<Replacement>,
        marks: Vec<std::ops::Range<usize>>,
        content: impl IntoElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_range(block, offset..offset + text.len(), &replacements);
        let matches = self.find_ranges(&text, &replacements);
        let background = layout
            .clone()
            .filter(|_| selected.is_some() || !matches.is_empty() || !marks.is_empty())
            .map(|layout| {
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        super::highlights::paint(
                            &layout,
                            &marks,
                            &matches,
                            selected.as_ref(),
                            window.text_style().text_align,
                            Theme::get(window, cx),
                            window,
                        );
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            });
        let hit = Hit {
            block,
            offset,
            text,
            layout,
            replacements: Rc::new(replacements),
            alignment: Rc::new(Cell::new(gpui::TextAlign::Left)),
            bounds: Bounds::default(),
        };
        let painted = hit.clone();
        let hits = self.reading.hits.clone();
        div()
            .relative()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.focus.focus(window);
                    let unit = match event.click_count {
                        1 => Unit::Character,
                        2 => Unit::Word,
                        _ => Unit::Block,
                    };
                    let target = hit.target(event.position, unit);
                    let anchor = match this.reading.selected {
                        Selection::Text { anchor, .. } if event.modifiers.shift => anchor..anchor,
                        _ => target.clone(),
                    };
                    this.reading.selected.extend(anchor.clone(), target);
                    this.reading.suppress_link = event.modifiers.shift || event.click_count > 1;
                    this.reading.drag = Some((anchor, unit, event.position));
                    this.selection_changed(cx);
                    cx.stop_propagation();
                }),
            )
            .children(background)
            .child(content)
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        painted.alignment.set(window.text_style().text_align);
                        let bounds = bounds.intersect(&window.content_mask().bounds);
                        if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                            hits.borrow_mut().push(Hit {
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
            .into_any_element()
    }

    pub(super) fn selection_layer(&self, cx: &Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let hits = self.reading.hits.clone();
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
                                this.reading.drag = None;
                            }
                        })
                        .ok();
                });
                let released = weak.clone();
                let frame_hits = hits.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        released
                            .update(cx, |this, cx| {
                                this.drag_selection(
                                    event.position,
                                    bounds,
                                    &frame_hits.borrow(),
                                    cx,
                                );
                                this.reading.drag = None;
                            })
                            .ok();
                    }
                });
                let pointer = window.mouse_position();
                let scrolling = weak
                    .upgrade()
                    .is_some_and(|v| v.read(cx).selection_scroll_step(pointer, bounds) != 0.);
                if scrolling {
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
                                this.reading.drag = None;
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
        hits: &[Hit],
        cx: &mut Context<Self>,
    ) {
        let Some((anchor, unit, start)) = self.reading.drag.clone() else {
            return;
        };
        if (pointer.x - start.x).abs() + (pointer.y - start.y).abs() < px(3.) {
            return;
        }
        self.reading.suppress_link = true;
        let gap =
            |p: Pixels, start: Pixels, end: Pixels| f32::from((start - p).max(p - end).max(px(0.)));
        let nearest = hits.iter().min_by(|a, b| {
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
        if let Some(hit) = nearest {
            let previous = self.reading.selected.clone();
            self.reading
                .selected
                .extend(anchor, hit.target(pointer, unit));
            if previous != self.reading.selected {
                self.selection_changed(cx);
            }
        }
        let distance = self.selection_scroll_step(pointer, bounds);
        if distance != 0. {
            self.list.scroll_by(px(distance * self.zoom));
            cx.notify();
        }
    }

    fn selection_scroll_step(&self, pointer: Point<Pixels>, bounds: Bounds<Pixels>) -> f32 {
        let Some((_, _, start)) = &self.reading.drag else {
            return 0.;
        };
        if (pointer.x - start.x).abs() + (pointer.y - start.y).abs() < px(3.) {
            return 0.;
        }
        let top = self.list.logical_scroll_top();
        if pointer.y < bounds.top() && (top.item_ix > 0 || top.offset_in_item > px(0.)) {
            -24.
        } else if pointer.y > bounds.bottom()
            && self
                .list
                .bounds_for_item(self.visible.len().saturating_sub(1))
                .is_none_or(|last| last.bottom() > bounds.bottom() + px(0.5))
        {
            24.
        } else {
            0.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formula_offsets_snap_to_tex_and_selection_covers_whole_formula() {
        let maps = [
            Replacement {
                source: 3..15,
                display: 3..8,
            },
            Replacement {
                source: 18..20,
                display: 11..31,
            },
        ];
        assert_eq!(source_offset(2, &maps), 2);
        assert_eq!(source_offset(4, &maps), 3);
        assert_eq!(source_offset(7, &maps), 15);
        assert_eq!(source_offset(9, &maps), 16);
        assert_eq!(source_offset(32, &maps), 21);
        assert_eq!(display_offset(4, false, &maps), 3);
        assert_eq!(display_offset(4, true, &maps), 8);
        assert_eq!(display_offset(15, true, &maps), 8);
        assert_eq!(display_offset(21, false, &maps), 32);
    }
}
