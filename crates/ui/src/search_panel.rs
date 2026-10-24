//! Project search (spec NAV-2), shown in the sidebar.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, Focusable, HighlightStyle, SharedString, StyledText,
    Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use plastext_editor::{TextInput, TextInputEvent, Theme};
use plastext_project::TreeOptions;
use plastext_search::{FileMatches, ProjectQuery, SearchOptions, search_project};

/// Searches stop after this many matching lines.
const LIMIT: usize = 10_000;
/// Typing pauses this long before a search starts.
const DEBOUNCE: Duration = Duration::from_millis(200);
const ROW_HEIGHT: f32 = 24.;

pub enum SearchPanelEvent {
    Open { path: PathBuf, line: u64 },
}

/// A row of the flattened results: a file, or one of its matching lines.
#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Line(usize, usize),
}

enum Progress {
    Idle,
    Searching,
    Done,
    Failed(String),
}

pub struct SearchPanel {
    root: PathBuf,
    tree_options: TreeOptions,
    input: Entity<TextInput>,
    options: SearchOptions,
    files: Vec<FileMatches>,
    rows: Vec<Row>,
    lines: usize,
    progress: Progress,
    cancel: Arc<AtomicBool>,
    search: Option<Task<()>>,
    scroll_handle: UniformListScrollHandle,
    _input_changes: Subscription,
}

impl EventEmitter<SearchPanelEvent> for SearchPanel {}

impl SearchPanel {
    pub fn new(root: PathBuf, tree_options: TreeOptions, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Search the project", cx));
        let input_changes = cx.subscribe(&input, |this, _, TextInputEvent::Changed, cx| {
            this.schedule_search(cx)
        });
        Self {
            root,
            tree_options,
            input,
            options: SearchOptions::default(),
            files: Vec::new(),
            rows: Vec::new(),
            lines: 0,
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
        self.files.clear();
        self.rows.clear();
        self.lines = 0;
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
        self.progress = Progress::Searching;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (files, mut received) = mpsc::unbounded();
        let root = self.root.clone();
        let tree_options = self.tree_options.clone();
        let searching = cx.background_executor().spawn(async move {
            search_project(&root, &tree_options, &query, LIMIT, &cancel, |found| {
                files.unbounded_send(found).ok();
            });
        });
        self.search = Some(cx.spawn(async move |this, cx| {
            while let Some(file) = received.next().await {
                if this.update(cx, |this, cx| this.add_file(file, cx)).is_err() {
                    return;
                }
            }
            searching.await;
            this.update(cx, |this, cx| {
                this.progress = Progress::Done;
                cx.notify();
            })
            .ok();
        }));
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn add_file(&mut self, file: FileMatches, cx: &mut Context<Self>) {
        let index = self.files.len();
        self.rows.push(Row::File(index));
        self.rows
            .extend((0..file.lines.len()).map(|line| Row::Line(index, line)));
        self.lines += file.lines.len();
        self.files.push(file);
        cx.notify();
    }

    fn open(&mut self, file: usize, line: usize, cx: &mut Context<Self>) {
        let file = &self.files[file];
        cx.emit(SearchPanelEvent::Open {
            path: self.root.join(&file.path),
            line: file.lines[line].line,
        });
    }

    fn summary(&self) -> String {
        let files = self.files.len();
        let found = format!(
            "{} in {files} {}",
            plural(self.lines, "result"),
            if files == 1 { "file" } else { "files" }
        );
        match &self.progress {
            Progress::Idle => String::new(),
            Progress::Searching => format!("Searching… {found}"),
            Progress::Done if self.lines >= LIMIT => format!("{found} (stopped at {LIMIT})"),
            Progress::Done if self.lines == 0 => "No results".to_owned(),
            Progress::Done => found,
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
        let theme = Theme::for_appearance(window.appearance());
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
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
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .child(
                toggle("whole-word", "ab", self.options.whole_word).on_click(cx.listener(
                    |this, _, _, cx| this.toggle(|options| options.whole_word ^= true, cx),
                )),
            )
            .child(toggle("regex", ".*", self.options.regex).on_click(
                cx.listener(|this, _, _, cx| this.toggle(|options| options.regex ^= true, cx)),
            ));

        let highlight = HighlightStyle {
            background_color: Some(theme.find_match.into()),
            ..Default::default()
        };
        let results = uniform_list(
            "search-results",
            self.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| match this.rows[index] {
                        Row::File(file) => {
                            let file = &this.files[file];
                            let (dir, name) = split_path(&file.path);
                            div()
                                .id(index)
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
                            let found = &this.files[file].lines[line];
                            let text = StyledText::new(found.text.clone()).with_highlights(
                                found.matches.iter().map(|range| (range.clone(), highlight)),
                            );
                            div()
                                .id(index)
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
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.summary()),
            )
            .child(results)
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
