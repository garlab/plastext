//! Project search (spec NAV-2), shown in the sidebar.

mod results;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui::{
    Context, CursorStyle, Entity, EventEmitter, Focusable, HighlightStyle, MouseButton,
    SharedString, StyledText, Subscription, Task, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};
use plastext_editor::{TextInput, TextInputEvent, Theme};
use plastext_project::TreeOptions;
use plastext_search::{FileFilters, FileMatches, ProjectQuery, ProjectSearch, SearchOptions};
use results::{Results, Row};

/// Matching lines per requested page.
const LIMIT: usize = 10_000;
/// Typing pauses this long before a search starts.
const DEBOUNCE: Duration = Duration::from_millis(200);
const UPDATE_INTERVAL: Duration = Duration::from_millis(16);
const RESULT_BATCH: usize = 256;
const ROW_HEIGHT: f32 = 24.;

pub enum SearchPanelEvent {
    Open { path: PathBuf, line: u64 },
}

enum Progress {
    Idle,
    Searching,
    Done { more: bool, incomplete: usize },
    Failed(String),
}

pub struct SearchPanel {
    root: PathBuf,
    tree_options: TreeOptions,
    input: Entity<TextInput>,
    include: Entity<TextInput>,
    exclude: Entity<TextInput>,
    filters_visible: bool,
    session: Option<ProjectSearch>,
    options: SearchOptions,
    results: Results,
    progress: Progress,
    cancel: Arc<AtomicBool>,
    search: Option<Task<()>>,
    scroll_handle: UniformListScrollHandle,
    _input_changes: Vec<Subscription>,
}

impl EventEmitter<SearchPanelEvent> for SearchPanel {}

impl SearchPanel {
    pub fn new(root: PathBuf, tree_options: TreeOptions, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Search the project", cx));
        let include = cx.new(|cx| TextInput::new("e.g. src/**, *.rs", cx));
        let exclude = cx.new(|cx| TextInput::new("e.g. target, **/*.min.js", cx));
        let input_changes = [&input, &include, &exclude]
            .map(|input| {
                cx.subscribe(input, |this, _, TextInputEvent::Changed, cx| {
                    this.schedule_search(cx)
                })
            })
            .into();
        Self {
            root,
            tree_options,
            input,
            include,
            exclude,
            filters_visible: false,
            session: None,
            options: SearchOptions::default(),
            results: Results::default(),
            progress: Progress::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            search: None,
            scroll_handle: UniformListScrollHandle::new(),
            _input_changes: input_changes,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.select_all_text(cx));
        self.input.focus_handle(cx).focus(window);
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        // Stop obsolete work immediately, including during the typing debounce.
        self.cancel.store(true, Ordering::Relaxed);
        self.session = None;
        self.progress = Progress::Searching;
        cx.notify();
        self.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            this.update(cx, |this, cx| this.start_search(cx)).ok();
        }));
    }

    fn toggle(&mut self, toggle: impl FnOnce(&mut SearchOptions), cx: &mut Context<Self>) {
        toggle(&mut self.options);
        self.start_search(cx);
    }

    /// Replaces the results with a fresh search for the current query.
    fn start_search(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.results = Results::default();
        self.session = None;
        self.search = None;
        let text = self.input.read(cx).text().to_owned();
        if text.is_empty() {
            self.progress = Progress::Idle;
            cx.notify();
            return;
        }
        let query = match ProjectQuery::new(&text, self.options) {
            Ok(query) => query,
            Err(err) => {
                self.progress = Progress::Failed(err);
                cx.notify();
                return;
            }
        };
        let filters =
            match FileFilters::new(self.include.read(cx).text(), self.exclude.read(cx).text()) {
                Ok(filters) => filters,
                Err(err) => {
                    self.progress = Progress::Failed(err);
                    cx.notify();
                    return;
                }
            };
        let root = self.root.clone();
        let tree_options = self.tree_options.clone();
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.run_page(
            move |cancel| ProjectSearch::new(&root, &tree_options, query, &filters, cancel),
            cx,
        );
    }

    fn show_more(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session.take() {
            self.run_page(move |_| session, cx);
        }
    }

    fn run_page(
        &mut self,
        create: impl FnOnce(&AtomicBool) -> ProjectSearch + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.progress = Progress::Searching;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (files, received) = async_channel::bounded(RESULT_BATCH);
        let worker_cancel = cancel.clone();
        let searching = cx.background_executor().spawn(async move {
            let mut session = create(&worker_cancel);
            let outcome = session.next_page(LIMIT, &worker_cancel, |found| {
                if files.send_blocking(found).is_err() {
                    worker_cancel.store(true, Ordering::Relaxed);
                }
            });
            (session, outcome)
        });
        self.search = Some(cx.spawn(async move |this, cx| {
            while let Ok(file) = received.recv().await {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let mut batch = vec![file];
                while batch.len() < RESULT_BATCH {
                    let Ok(file) = received.try_recv() else { break };
                    batch.push(file);
                }
                if this
                    .update(cx, |this, cx| {
                        if Arc::ptr_eq(&cancel, &this.cancel) && !cancel.load(Ordering::Relaxed) {
                            this.add_files(batch, cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
                // First results publish immediately; subsequent bursts share an update.
                cx.background_executor().timer(UPDATE_INTERVAL).await;
            }
            let (session, outcome) = searching.await;
            this.update(cx, |this, cx| {
                if Arc::ptr_eq(&cancel, &this.cancel) && !cancel.load(Ordering::Relaxed) {
                    match outcome {
                        Ok(more) => {
                            this.progress = Progress::Done {
                                more,
                                incomplete: session.incomplete_files(),
                            };
                            this.session = more.then_some(session);
                        }
                        Err(error) => this.progress = Progress::Failed(error),
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn add_files(&mut self, files: Vec<FileMatches>, cx: &mut Context<Self>) {
        let scroll = self.scroll_handle.0.borrow().base_handle.clone();
        let offset = scroll.offset();
        let top = (f32::from(-offset.y) / ROW_HEIGHT).floor().max(0.) as usize;
        let anchor = self.results.rows.get(top).copied();
        self.results.extend(files);
        // If the reader scrolled down, hold the same match at the same pixel
        // position when alphabetically earlier files arrive. At the top, show
        // the new first result instead.
        if offset.y < px(0.)
            && let Some(anchor) = anchor
            && let Some(next) = self.results.rows.iter().position(|row| *row == anchor)
        {
            scroll.set_offset(gpui::point(
                offset.x,
                offset.y - px((next as f32 - top as f32) * ROW_HEIGHT),
            ));
        }
        cx.notify();
    }

    fn open(&mut self, file: usize, line: usize, cx: &mut Context<Self>) {
        let file = &self.results.files[file];
        cx.emit(SearchPanelEvent::Open {
            path: self.root.join(&file.path),
            line: file.lines[line].line,
        });
    }

    fn summary(&self) -> String {
        let files = self.results.files.len();
        let found = format!(
            "{} in {files} {}",
            plural(self.results.lines, "result"),
            if files == 1 { "file" } else { "files" }
        );
        match &self.progress {
            Progress::Idle => String::new(),
            Progress::Searching => format!("Searching… {found}"),
            Progress::Done { more, incomplete } => {
                let mut text = if self.results.lines == 0 {
                    "No results".to_owned()
                } else {
                    found
                };
                if *more {
                    text.push_str(" · Search paused");
                }
                if *incomplete > 0 {
                    text.push_str(&format!(
                        " · {} not fully searched",
                        plural(*incomplete, "file")
                    ));
                }
                text
            }
            Progress::Failed(err) => err.clone(),
        }
    }
}

impl Drop for SearchPanel {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let toggle = |id: &'static str, label: &'static str, help: &'static str, on: bool| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .text_xs()
                .tooltip(move |_, cx| cx.new(|_| SearchTooltip(help)).into())
                .when(on, |toggle| {
                    toggle.bg(theme.selection).text_color(theme.text)
                })
                .when(!on, |toggle| {
                    toggle
                        .text_color(theme.text_muted)
                        .hover(|toggle| toggle.bg(theme.border))
                })
                .child(label)
        };
        let input = div()
            .m_2()
            .px_2()
            .py_1()
            .flex()
            .items_center()
            .gap_1()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.input.focus_handle(cx).focus(window)),
            )
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .child(
                toggle(
                    "match-case",
                    "Aa",
                    "Match Case",
                    self.options.case_sensitive,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle(|options| options.case_sensitive ^= true, cx)
                })),
            )
            .child(
                toggle(
                    "whole-word",
                    "ab",
                    "Match Whole Word",
                    self.options.whole_word,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle(|options| options.whole_word ^= true, cx)
                })),
            )
            .child(
                toggle("regex", ".*", "Use Regular Expression", self.options.regex).on_click(
                    cx.listener(|this, _, _, cx| this.toggle(|options| options.regex ^= true, cx)),
                ),
            );

        let highlight = HighlightStyle {
            background_color: Some(theme.find_match.into()),
            ..Default::default()
        };
        let results = uniform_list(
            "search-results",
            self.results.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| match this.results.rows[index] {
                        Row::File(file) => {
                            let file_id = file;
                            let file = &this.results.files[file];
                            let (dir, name) = split_path(&file.path);
                            div()
                                .id(("search-file", file_id))
                                .h(px(ROW_HEIGHT))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_none().text_color(theme.text).child(name))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(dir),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(file.lines.len().to_string()),
                                )
                        }
                        Row::Line(file, line) => {
                            let found = &this.results.files[file].lines[line];
                            let text = StyledText::new(found.text.clone()).with_highlights(
                                found.matches.iter().map(|range| (range.clone(), highlight)),
                            );
                            div()
                                .id(SharedString::from(format!("search-line-{file}-{line}")))
                                .h(px(ROW_HEIGHT))
                                .pl_4()
                                .pr_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .hover(|row| row.bg(theme.background))
                                .child(
                                    div()
                                        .w(px(36.))
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(found.line.to_string()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(text),
                                )
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.open(file, line, cx)),
                                )
                        }
                    })
                    .collect()
            }),
        )
        .track_scroll(self.scroll_handle.clone())
        .flex_1();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(input)
            .child(
                div()
                    .id("search-filters-toggle")
                    .mx_2()
                    .pb_1()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .child(if self.filters_visible {
                        "Hide file filters"
                    } else if !self.include.read(cx).text().is_empty()
                        || !self.exclude.read(cx).text().is_empty()
                    {
                        "File filters · active"
                    } else {
                        "File filters"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters_visible ^= true;
                        cx.notify();
                    })),
            )
            .when(self.filters_visible, |panel| {
                let field = |label: &'static str, input: Entity<TextInput>| {
                    let focus = input.focus_handle(cx);
                    div()
                        .mx_2()
                        .mb_2()
                        .text_xs()
                        .child(div().text_color(theme.text_muted).child(label))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .bg(theme.background)
                                .border_1()
                                .border_color(theme.border)
                                .rounded_sm()
                                .cursor(CursorStyle::IBeam)
                                .on_mouse_down(MouseButton::Left, move |_, window, _| {
                                    focus.focus(window)
                                })
                                .child(input),
                        )
                };
                panel
                    .child(field("Files to include", self.include.clone()))
                    .child(field("Files to exclude", self.exclude.clone()))
            })
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.summary()),
            )
            .child(results)
            .when(
                matches!(self.progress, Progress::Done { more: true, .. }),
                |panel| {
                    panel.child(
                        div()
                            .id("search-show-more")
                            .m_2()
                            .p_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(theme.text)
                            .hover(|button| button.bg(theme.border))
                            .child("Show more")
                            .on_click(cx.listener(|this, _, _, cx| this.show_more(cx))),
                    )
                },
            )
    }
}

struct SearchTooltip(&'static str);
impl Render for SearchTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        div()
            .p_2()
            .rounded_sm()
            .bg(theme.surface)
            .text_color(theme.text)
            .text_xs()
            .child(self.0)
    }
}

/// A relative path as (its directory, its file name).
fn split_path(path: &Path) -> (SharedString, SharedString) {
    let name = path
        .file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
    let dir = path
        .parent()
        .map(|dir| dir.to_string_lossy())
        .unwrap_or_default();
    (dir.into_owned().into(), name.into_owned().into())
}
