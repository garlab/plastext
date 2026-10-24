use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, ScrollStrategy, SharedString,
    Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
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
const PAGE: u64 = 64;
const HEIGHT: f32 = 28.;

pub struct CsvView {
    path: PathBuf,
    source: Arc<FileSource>,
    lines: Arc<LineIndex>,
    index: Arc<CsvIndex>,
    delimiter: u8,
    style: TextStyle,
    pages: HashMap<u64, Vec<CsvRow>>,
    order: VecDeque<u64>,
    loading: HashSet<u64>,
    widths: Vec<f32>,
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
            pages: HashMap::new(),
            order: VecDeque::new(),
            loading: HashSet::new(),
            widths: Vec::new(),
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
        }
    }
    fn request(&mut self, page: u64, cx: &mut Context<Self>) {
        if self.pages.contains_key(&page) || self.loading.contains(&page) || self.loading.len() >= 4
        {
            return;
        }
        self.loading.insert(page);
        let source = self.source.clone();
        let index = self.index.clone();
        let delimiter = self.delimiter;
        let reading = cx
            .background_executor()
            .spawn(async move { index.page(&source, delimiter, page * PAGE, PAGE as usize) });
        cx.spawn(async move |this, cx| {
            let rows = reading.await;
            this.update(cx, |this, cx| {
                this.loading.remove(&page);
                match rows {
                    Ok(rows) => {
                        if page == 0 {
                            for row in rows.iter().take(32) {
                                if let Ok(cells) = &row.cells {
                                    this.widths.resize(this.widths.len().max(cells.len()), 100.);
                                    for (i, cell) in cells.iter().enumerate() {
                                        this.widths[i] = this.widths[i].max(
                                            (cell.chars().count().min(48) as f32 * 7.5 + 24.)
                                                .min(380.),
                                        );
                                    }
                                }
                            }
                        }
                        this.pages.insert(page, rows);
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request(number / PAGE, cx);
        let record = self
            .pages
            .get(&(number / PAGE))
            .and_then(|rows| rows.get((number % PAGE) as usize));
        let cells = match record {
            Some(CsvRow {
                cells: Ok(cells), ..
            }) => cells.clone(),
            Some(CsvRow {
                cells: Err(error), ..
            }) => vec![error.clone()],
            None => vec!["Loading…".into()],
        };
        div()
            .h(px(HEIGHT))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .when(header, |d| {
                d.bg(theme.surface).font_weight(gpui::FontWeight::BOLD)
            })
            .child(
                div()
                    .w(px(56.))
                    .flex_none()
                    .pl_2()
                    .text_color(theme.text_muted)
                    .child(if header {
                        "#".into()
                    } else {
                        number.to_string()
                    }),
            )
            .children(cells.into_iter().enumerate().map(|(i, cell)| {
                div()
                    .w(px(self.widths.get(i).copied().unwrap_or(160.)))
                    .flex_none()
                    .px_2()
                    .overflow_hidden()
                    .text_ellipsis()
                    .border_l_1()
                    .border_color(theme.border)
                    .child(cell.replace(['\r', '\n'], " ↵ "))
            }))
            .into_any_element()
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
        self.request(0, cx);
        let count = self.index.progress().rows.saturating_sub(1) as usize;
        let header = self.row(0, true, theme, cx);
        let width = self.widths.iter().sum::<f32>() + 56.;
        let body = if self.show_raw {
            self.raw.as_ref().unwrap().clone().into_any_element()
        } else {
            div()
                .id("csv-horizontal")
                .size_full()
                .overflow_x_scroll()
                .child(
                    div()
                        .h_full()
                        .w(px(width.max(640.)))
                        .flex()
                        .flex_col()
                        .child(header)
                        .child(
                            uniform_list(
                                "csv-rows",
                                count,
                                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                    this.first_row = range.start as u64;
                                    range
                                        .map(|i| this.row(i as u64 + 1, false, theme, cx))
                                        .collect()
                                }),
                            )
                            .track_scroll(self.scroll.clone())
                            .flex_1(),
                        ),
                )
                .into_any_element()
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_sm()
            .text_color(theme.text)
            .child(
                div()
                    .h(px(30.))
                    .flex()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .text_xs()
                    .child(self.summary())
                    .child(
                        div()
                            .id("csv-raw")
                            .cursor_pointer()
                            .child(if self.show_raw { "Table" } else { "Source" })
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
