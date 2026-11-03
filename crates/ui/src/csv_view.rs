use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Focusable,
    FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollHandle,
    ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle, Window, canvas, div,
    prelude::*, px, uniform_list,
};
use plastext_assets::csv::{CsvIndex, CsvRow};
use plastext_buffer::{FileSource, LineIndex};
use plastext_editor::{TextStyle, TextView, Theme};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
mod columns;
mod copy;
mod interaction;
mod profile;
mod selection;
use columns::{ColumnDrag, Columns, FONT_SIZE, sample_widths};
pub(crate) use interaction::init;
use interaction::{ClearSelection, FrameHits};
use selection::{Drag, Selection};

const PAGE: u64 = 64;
const HEIGHT: f32 = 28.;

pub struct CsvView {
    path: PathBuf,
    source: Arc<FileSource>,
    lines: Arc<LineIndex>,
    index: Arc<CsvIndex>,
    delimiter: u8,
    style: TextStyle,
    zoom: f32,
    pages: HashMap<u64, Vec<CsvRow>>,
    order: VecDeque<u64>,
    loading: HashSet<u64>,
    columns: Columns,
    column_drag: Option<ColumnDrag>,
    selection: Selection,
    selection_drag: Option<Drag>,
    horizontal_scroll: ScrollHandle,
    copy_cancel: Arc<AtomicBool>,
    _copy: Option<Task<()>>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    first_row: u64,
    raw: Option<Entity<TextView>>,
    show_raw: bool,
    pending_offset: Option<u64>,
    _navigation: Option<Task<()>>,
    _raw_observer: Option<Subscription>,
    notice: Option<SharedString>,
    cancel: Arc<AtomicBool>,
    _indexing: Task<()>,
    profile: Option<profile::ReadingProfile>,
}
impl CsvView {
    pub fn new(
        path: PathBuf,
        source: Arc<FileSource>,
        lines: Arc<LineIndex>,
        style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let delimiter = if path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("tsv"))
        {
            b'\t'
        } else {
            b','
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, receive) = async_channel::bounded(1);
        let index = CsvIndex::start(
            source.clone(),
            delimiter,
            cancel.clone(),
            Arc::new(move || {
                send.try_send(()).ok();
            }),
        );
        let indexing = cx.spawn(async move |this, cx| {
            while receive.recv().await.is_ok() {
                let finished = this.update(cx, |this, cx| {
                    this.notice = this.index.error().map(Into::into);
                    if let Some(offset) = this.pending_offset {
                        this.scroll_to_offset(offset, cx);
                    }
                    cx.notify();
                    this.index.progress().complete
                });
                if !matches!(finished, Ok(false)) {
                    break;
                }
            }
        });
        Self {
            path,
            source,
            lines,
            index,
            delimiter,
            style,
            zoom: 1.,
            pages: HashMap::new(),
            order: VecDeque::new(),
            loading: HashSet::new(),
            columns: Columns::default(),
            column_drag: None,
            selection: Selection::None,
            selection_drag: None,
            horizontal_scroll: ScrollHandle::new(),
            copy_cancel: Arc::new(AtomicBool::new(false)),
            _copy: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            first_row: 0,
            raw: None,
            show_raw: false,
            pending_offset: None,
            _navigation: None,
            _raw_observer: None,
            notice: None,
            cancel,
            _indexing: indexing,
            profile: None,
        }
    }
    fn request(&mut self, page: u64, window: &Window, cx: &mut Context<Self>) {
        if self.pages.contains_key(&page) || self.loading.contains(&page) || self.loading.len() >= 4
        {
            return;
        }
        self.loading.insert(page);
        let source = self.source.clone();
        let index = self.index.clone();
        let delimiter = self.delimiter;
        let metrics = (page == 0).then(|| (cx.text_system().clone(), window.text_style().font()));
        let reading = cx.background_executor().spawn(async move {
            let rows = index.page(&source, delimiter, page * PAGE, PAGE as usize);
            let widths = metrics.and_then(|(text_system, mut font)| {
                let rows = rows.as_ref().ok()?;
                font.weight = FontWeight::NORMAL;
                let regular = text_system.resolve_font(&font);
                font.weight = FontWeight::BOLD;
                let bold = text_system.resolve_font(&font);
                let mut advances = HashMap::new();
                Some(sample_widths(rows, |ch, header| {
                    *advances.entry((ch, header)).or_insert_with(|| {
                        text_system
                            .advance(if header { bold } else { regular }, px(FONT_SIZE), ch)
                            .map_or(FONT_SIZE, |size| f32::from(size.width))
                    })
                }))
            });
            (rows, widths)
        });
        cx.spawn(async move |this, cx| {
            let (rows, widths) = reading.await;
            this.update(cx, |this, cx| {
                this.loading.remove(&page);
                match rows {
                    Ok(rows) => {
                        if let Some(widths) = widths {
                            this.columns.set_sampled(widths);
                        }
                        let count = rows
                            .iter()
                            .filter_map(|row| row.cells.as_ref().ok().map(Vec::len))
                            .max()
                            .unwrap_or(0);
                        this.columns.ensure_count(count);
                        this.pages.insert(page, rows);
                        if let Some(profile) = &mut this.profile {
                            profile.loaded_page(page);
                        }
                        this.order.retain(|&p| p != page);
                        this.order.push_back(page);
                        while this.order.len() > 8 {
                            if let Some(old) = this.order.pop_front()
                                && old != 0
                            {
                                this.pages.remove(&old);
                            }
                        }
                    }
                    Err(error) => {
                        if this.index.progress().complete {
                            this.notice = Some(error.to_string().into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn row(
        &mut self,
        number: u64,
        header: bool,
        theme: Theme,
        hits: FrameHits,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request(number / PAGE, window, cx);
        let record = self
            .pages
            .get(&(number / PAGE))
            .and_then(|rows| rows.get((number % PAGE) as usize));
        let selectable = record.is_some_and(|row| row.cells.is_ok());
        let mut cells = match record {
            Some(CsvRow {
                cells: Ok(cells), ..
            }) => cells.clone(),
            Some(CsvRow {
                cells: Err(error), ..
            }) => vec![error.clone()],
            None => vec!["Loading…".into()],
        };
        // Ragged records still need a header divider and aligned empty cells.
        cells.resize(cells.len().max(self.columns.len()), String::new());
        div()
            .id(("csv-row", number as usize))
            .h(px(HEIGHT * self.zoom))
            .flex_none()
            .text_size(px(FONT_SIZE * self.zoom))
            .font_weight(FontWeight::NORMAL)
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .when(header, |d| {
                d.bg(theme.surface).font_weight(FontWeight::BOLD)
            })
            .when(self.selection.row_selected(number), |row| {
                row.bg(theme.selection)
            })
            .child(
                div()
                    .id("row-number")
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.select_row(number, event, window, cx);
                        }),
                    )
                    .w(px(56. * self.zoom))
                    .flex_none()
                    .pl(px(8. * self.zoom))
                    .text_color(theme.text_muted)
                    .child(if header {
                        "#".into()
                    } else {
                        number.to_string()
                    }),
            )
            .children(cells.into_iter().enumerate().map(|(i, cell)| {
                self.render_cell(number, i, cell, selectable, theme, hits.clone(), cx)
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn resize_overlay(&self, cx: &Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .cursor(CursorStyle::ResizeLeftRight)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moved = weak.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            moved
                                .update(cx, |this, cx| {
                                    if let Some(drag) = this.column_drag {
                                        if event.dragging() {
                                            this.columns.resize(
                                                drag.column,
                                                drag.width_at(event.position.x.into(), this.zoom),
                                            );
                                        } else {
                                            // Recover from a release outside the window too.
                                            this.column_drag = None;
                                        }
                                        cx.notify();
                                    }
                                })
                                .ok();
                        });
                        let released = weak.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                                released
                                    .update(cx, |this, cx| {
                                        this.column_drag = None;
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
    pub fn summary(&self) -> String {
        let p = self.index.progress();
        format!(
            "{} rows{}    {}",
            p.rows.saturating_sub(1),
            if p.complete { "" } else { " indexed…" },
            if self.delimiter == b'\t' {
                "TSV"
            } else {
                "CSV"
            }
        )
    }
    pub fn notice(&self) -> Option<SharedString> {
        self.notice.clone()
    }
    pub fn position(&self, cx: &App) -> u64 {
        if self.show_raw
            && let Some(raw) = &self.raw
        {
            return raw.read(cx).position();
        }
        let row = self.first_row + 1;
        self.pages
            .get(&(row / PAGE))
            .and_then(|page| page.get((row % PAGE) as usize))
            .map_or(0, |row| row.source.start)
    }
    pub fn scroll_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        if let Ok(Some(offset)) = self.lines.line_start(line.saturating_sub(1), &*self.source) {
            self.scroll_to_offset(offset, cx);
        }
    }
    pub fn scroll_to_offset(&mut self, offset: u64, cx: &mut Context<Self>) {
        if let Some(raw) = &self.raw {
            raw.update(cx, |v, cx| v.scroll_to_offset(offset, cx));
        }
        let progress = self.index.progress();
        if !progress.complete && offset >= progress.bytes {
            self.pending_offset = Some(offset);
            return;
        }
        self.pending_offset = None;
        let index = self.index.clone();
        let source = self.source.clone();
        let delimiter = self.delimiter;
        let lookup = cx
            .background_executor()
            .spawn(async move { index.row_for_offset(&source, delimiter, offset) });
        self._navigation = Some(cx.spawn(async move |this, cx| {
            if let Ok(row) = lookup.await {
                this.update(cx, |this, cx| {
                    this.first_row = row.saturating_sub(1);
                    this.scroll
                        .scroll_to_item(this.first_row as usize, ScrollStrategy::Top);
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }
    pub fn set_zoom(&mut self, factor: f32, font_size: gpui::Pixels, cx: &mut Context<Self>) {
        self.style.font_size = font_size;
        if let Some(raw) = &self.raw {
            raw.update(cx, |view, cx| view.set_font_size(font_size, cx));
        }
        if self.zoom != factor {
            self.selection_drag = None;
            self.column_drag = None;
            self.zoom = factor;
            self.scroll
                .scroll_to_item(self.first_row as usize, ScrollStrategy::Top);
            cx.notify();
        }
    }

    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let path = self.path.clone();
        let source = self.source.clone();
        let lines = self.lines.clone();
        let style = self.style.clone();
        let first = self.first_row;
        let show_raw = self.show_raw;
        let raw = self
            .raw
            .as_ref()
            .map(|v| v.update(cx, |v, cx| v.duplicate(cx)));
        cx.new(|cx| {
            let mut view = Self::new(path, source, lines, style, cx);
            view.columns = self.columns.clone();
            view.selection = self.selection.clone();
            view.zoom = self.zoom;
            view.first_row = first;
            view.show_raw = show_raw;
            view.raw = raw;
            if let Some(raw) = &view.raw {
                view._raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
            view.scroll
                .scroll_to_item(first as usize, ScrollStrategy::Top);
            view
        })
    }
    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.column_drag = None;
        self.selection_drag = None;
        self.selection_changed(cx);
        if !self.show_raw {
            let offset = self.position(cx);
            let raw = self.raw.get_or_insert_with(|| {
                cx.new(|cx| {
                    TextView::new(
                        self.source.clone(),
                        self.lines.clone(),
                        self.style.clone(),
                        None,
                        cx,
                    )
                })
            });
            if self._raw_observer.is_none() {
                self._raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
            raw.update(cx, |v, cx| v.scroll_to_offset(offset, cx));
            raw.focus_handle(cx).focus(window);
        } else {
            if let Some(raw) = &self.raw {
                self.scroll_to_offset(raw.read(cx).position(), cx);
            }
            self.focus.focus(window);
        }
        self.show_raw = !self.show_raw;
        cx.notify();
    }
}
impl Drop for CsvView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.copy_cancel.store(true, Ordering::Relaxed);
    }
}
impl Focusable for CsvView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.show_raw
            && let Some(raw) = &self.raw
        {
            raw.focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }
}
impl Render for CsvView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        self.request(0, window, cx);
        let count = self.index.progress().rows.saturating_sub(1) as usize;
        let hits = FrameHits::default();
        let list_hits = hits.clone();
        let header = self.row(0, true, theme, hits.clone(), window, cx);
        let width = (self.columns.total_width() + 56.).max(640.) * self.zoom;
        let body = if self.show_raw {
            self.raw.as_ref().unwrap().clone().into_any_element()
        } else {
            div()
                .id("csv-horizontal")
                .size_full()
                .overflow_x_scroll()
                .track_scroll(&self.horizontal_scroll)
                .child(
                    div()
                        .h_full()
                        .w(px(width))
                        .flex()
                        .flex_col()
                        .child(header)
                        .child(
                            uniform_list(
                                "csv-rows",
                                count,
                                cx.processor(
                                    move |this, range: std::ops::Range<usize>, window, cx| {
                                        this.first_row = range.start as u64;
                                        if let Some(profile) = &mut this.profile {
                                            profile.rendered_rows(range.clone(), &this.pages);
                                        }
                                        let rows = range
                                            .clone()
                                            .map(|i| {
                                                this.row(
                                                    i as u64 + 1,
                                                    false,
                                                    theme,
                                                    list_hits.clone(),
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .collect::<Vec<_>>();
                                        // Visible reads have priority; keep one page on
                                        // either side warm within the same cache/job caps.
                                        // The one-row sizing probe is not a viewport.
                                        if range.len() > 1 {
                                            let first = (range.start as u64 + 1) / PAGE;
                                            let last = range.end as u64 / PAGE;
                                            let progress = this.index.progress();
                                            let pages = if progress.complete {
                                                progress.rows.div_ceil(PAGE)
                                            } else {
                                                progress.rows / PAGE
                                            };
                                            if last + 1 < pages {
                                                this.request(last + 1, window, cx);
                                            }
                                            if let Some(previous) = first.checked_sub(1) {
                                                this.request(previous, window, cx);
                                            }
                                        }
                                        rows
                                    },
                                ),
                            )
                            .track_scroll(self.scroll.clone())
                            .flex_1(),
                        ),
                )
                .into_any_element()
        };
        div()
            .relative()
            .track_focus(&self.focus)
            .when(!self.show_raw, |view| {
                view.key_context("CsvTable")
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::select_all))
                    .on_action(cx.listener(Self::clear_selection))
            })
            .size_full()
            .flex()
            .flex_col()
            .when(self.profile.is_some(), |view| {
                let weak = cx.entity().downgrade();
                view.child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, cx| {
                            weak.update(cx, |this, cx| this.profile_painted(window, cx))
                                .ok();
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .text_sm()
            .text_color(theme.text)
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .text_xs()
                    .child(if self._copy.is_some() {
                        format!("{}    Copying…", self.summary())
                    } else {
                        self.summary()
                    })
                    .child(
                        div()
                            .id("csv-raw")
                            .cursor_pointer()
                            .child(if self.show_raw { "Table" } else { "Source" })
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .when(!self.show_raw, |body| {
                        body.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.focus.focus(window);
                                this.clear_selection(&ClearSelection, window, cx);
                            }),
                        )
                        .child(self.selection_layer(hits, cx))
                    }),
            )
            .when(self.column_drag.is_some(), |view| {
                view.child(self.resize_overlay(cx))
            })
    }
}
