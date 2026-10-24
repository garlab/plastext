//! Quick Open (spec NAV-1): fuzzy-find any file in the project.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ScrollStrategy,
    SharedString, Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px,
    uniform_list,
};
use plastext_editor::{TextInput, TextInputEvent, Theme};
use plastext_project::{TreeOptions, walk_files};
use plastext_search::FileMatcher;

actions!(quick_open, [SelectNext, SelectPrevious, Confirm, Dismiss]);

const KEY_CONTEXT: &str = "QuickOpen";
const WIDTH: f32 = 640.;
const ROW_HEIGHT: f32 = 30.;
const VISIBLE_ROWS: usize = 12;
/// How often results refresh while files are found or matched.
const TICK: Duration = Duration::from_millis(16);

pub fn init(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("ctrl-n", SelectNext, context),
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("ctrl-p", SelectPrevious, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("escape", Dismiss, context),
    ]);
}

pub enum QuickOpenEvent {
    Open {
        path: PathBuf,
        line: Option<u64>,
    },
    /// A bare `:line` query: go to that line in the current file (spec NAV-4).
    GoToLine(u64),
    Dismissed,
}

/// The ⌘P palette. Files stream in from a background walk of the project
/// while nucleo matches them against the query on its own threads.
pub struct QuickOpen {
    root: PathBuf,
    input: Entity<TextInput>,
    matcher: FileMatcher,
    /// Set while the background walk is still finding files.
    walking: Arc<AtomicBool>,
    cancel_walk: Arc<AtomicBool>,
    ticking: bool,
    pattern: String,
    line: Option<u64>,
    /// Recently opened files, relative to the root, most recent first.
    recent: Vec<String>,
    selected: usize,
    scroll_handle: UniformListScrollHandle,
    _input_changes: Subscription,
}

impl EventEmitter<QuickOpenEvent> for QuickOpen {}

impl QuickOpen {
    /// A palette over the files under `root`, listing `recent` files (most
    /// recent first), also boosting them when they match a typed query.
    pub fn new(
        root: PathBuf,
        options: TreeOptions,
        recent: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let recent: Vec<String> = recent
            .iter()
            .filter_map(|path| path.strip_prefix(&root).ok())
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let input = cx.new(|cx| TextInput::new("Go to file (path:line works too)", cx));
        let input_changes = cx.subscribe(&input, |this, _, TextInputEvent::Changed, cx| {
            this.query_changed(cx)
        });
        input.focus_handle(cx).focus(window);

        let matcher = FileMatcher::new(&recent);
        let injector = matcher.injector();
        let walking = Arc::new(AtomicBool::new(true));
        let cancel_walk = Arc::new(AtomicBool::new(false));
        cx.background_executor()
            .spawn({
                let root = root.clone();
                let walking = walking.clone();
                let cancel = cancel_walk.clone();
                async move {
                    walk_files(&root, &options, &cancel, |path| {
                        let path = path.to_string_lossy().into_owned();
                        injector.push(path);
                    });
                    walking.store(false, Ordering::Relaxed);
                }
            })
            .detach();

        let mut this = Self {
            root,
            input,
            matcher,
            walking,
            cancel_walk,
            ticking: false,
            pattern: String::new(),
            line: None,
            recent,
            selected: 0,
            scroll_handle: UniformListScrollHandle::new(),
            _input_changes: input_changes,
        };
        this.keep_ticking(cx);
        this
    }

    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_text(query.to_owned(), cx));
    }

    /// Whether the recent files are listed, rather than matches.
    fn showing_recent(&self) -> bool {
        self.pattern.is_empty() && self.line.is_none() && !self.recent.is_empty()
    }

    fn count(&self) -> usize {
        if self.showing_recent() {
            self.recent.len()
        } else {
            self.matcher.matched_count()
        }
    }

    /// The listed path at `index`, relative to the root.
    fn item(&self, index: usize) -> Option<&str> {
        if self.showing_recent() {
            self.recent.get(index).map(String::as_str)
        } else {
            self.matcher.item(index)
        }
    }

    /// A line to go to in the current file, when the query is only `:line`.
    fn go_to_line(&self) -> Option<u64> {
        self.line.filter(|_| self.pattern.is_empty())
    }

    /// Refreshes results until the walk and the matching are both done.
    fn keep_ticking(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let busy = this.update(cx, |this, cx| {
                    let status = this.matcher.tick();
                    if status.changed {
                        this.selected = this.selected.min(this.count().saturating_sub(1));
                        cx.notify();
                    }
                    let busy = status.running || this.walking.load(Ordering::Relaxed);
                    this.ticking = busy;
                    busy
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let (pattern, line) = split_line(self.input.read(cx).text());
        let append = pattern.starts_with(&self.pattern);
        self.matcher.reparse(pattern, append);
        self.pattern = pattern.to_owned();
        self.line = line;
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        self.keep_ticking(cx);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.count();
        if count > 0 {
            self.select(((self.selected + 1) % count) as isize, cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.count() as isize;
        if count > 0 {
            self.select((self.selected as isize - 1).rem_euclid(count), cx);
        }
    }

    fn select(&mut self, index: isize, cx: &mut Context<Self>) {
        self.selected = index as usize;
        self.scroll_handle
            .scroll_to_item(self.selected, ScrollStrategy::Top);
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        match self.go_to_line() {
            Some(line) => cx.emit(QuickOpenEvent::GoToLine(line)),
            None => self.open(self.selected, cx),
        }
    }

    fn open(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self.item(index) else {
            return;
        };
        let path = self.root.join(item);
        cx.emit(QuickOpenEvent::Open {
            path,
            line: self.line,
        });
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(QuickOpenEvent::Dismissed);
    }
}

impl Drop for QuickOpen {
    fn drop(&mut self) {
        self.cancel_walk.store(true, Ordering::Relaxed);
    }
}

impl Focusable for QuickOpen {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for QuickOpen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let matched = self.count();
        let total = self.matcher.item_count();
        let footer = if self.showing_recent() {
            format!("Recently opened · {total} files")
        } else if self.walking.load(Ordering::Relaxed) {
            format!("{matched} of {total} files so far…")
        } else {
            format!("{matched} of {total} files")
        };
        let results = if let Some(line) = self.go_to_line() {
            div()
                .h(px(ROW_HEIGHT))
                .px_3()
                .flex()
                .items_center()
                .bg(theme.selection)
                .child(format!("Go to line {line} in the current file"))
                .into_any_element()
        } else if self.pattern.is_empty() && self.input.read(cx).text().trim() == ":" {
            div()
                .h(px(ROW_HEIGHT))
                .px_3()
                .flex()
                .items_center()
                .text_color(theme.text_muted)
                .child("Type a line number")
                .into_any_element()
        } else if matched == 0 {
            div()
                .h(px(ROW_HEIGHT))
                .px_3()
                .flex()
                .items_center()
                .text_color(theme.text_muted)
                .child("No matching files")
                .into_any_element()
        } else {
            uniform_list(
                "quick-open-results",
                matched,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let (dir, name) = split_path(this.item(index)?);
                            let selected = index == this.selected;
                            Some(
                                div()
                                    .id(index)
                                    .h(px(ROW_HEIGHT))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .when(selected, |row| row.bg(theme.selection))
                                    .child(div().flex_none().text_color(theme.text).child(name))
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_color(theme.text_muted)
                                            .child(dir),
                                    )
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.open(index, cx)),
                                    ),
                            )
                        })
                        .collect()
                }),
            )
            .track_scroll(self.scroll_handle.clone())
            .h(px(ROW_HEIGHT * matched.min(VISIBLE_ROWS) as f32))
            .into_any_element()
        };

        div()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(QuickOpenEvent::Dismissed)))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .text_sm()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.input.clone()),
            )
            .child(results)
            .child(
                div()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(footer),
            )
    }
}

/// Splits a trailing `:line` or `:line:column` off a query.
fn split_line(query: &str) -> (&str, Option<u64>) {
    let query = query.trim();
    let number = |text: &str| text.parse::<u64>().ok();
    let Some((rest, last)) = query
        .rsplit_once(':')
        .filter(|(_, last)| number(last).is_some())
    else {
        return (query, None);
    };
    match rest.rsplit_once(':') {
        Some((path, line)) if number(line).is_some() => (path, number(line)),
        _ => (rest, number(last)),
    }
}

/// A relative path as (its directory, its file name).
fn split_path(path: &str) -> (SharedString, SharedString) {
    let path = Path::new(path);
    let name = path
        .file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
    let dir = path
        .parent()
        .map(|dir| dir.to_string_lossy())
        .unwrap_or_default();
    (dir.into_owned().into(), name.into_owned().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_line_numbers_off_queries() {
        assert_eq!(split_line("main.rs"), ("main.rs", None));
        assert_eq!(split_line("main.rs:42"), ("main.rs", Some(42)));
        assert_eq!(split_line("main.rs:042"), ("main.rs", Some(42)));
        assert_eq!(split_line("src/main.rs:42:7"), ("src/main.rs", Some(42)));
        assert_eq!(split_line(" lib "), ("lib", None));
        assert_eq!(split_line("a:b"), ("a:b", None));
        assert_eq!(split_line(":12"), ("", Some(12)));
    }
}
