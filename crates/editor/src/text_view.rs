use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, ContentMask, Context, DispatchPhase, Entity,
    FocusHandle, Focusable, Font, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Rgba, ScrollWheelEvent, ShapedLine, SharedString, Subscription, Task,
    TextRun, Window, WindowTextSystem, actions, canvas, div, fill, font, point, prelude::*, px,
    size,
};
use plastext_buffer::{ByteSource, FileSource, LineIndex, ReadError, rows};
use plastext_search::Query;
use plastext_syntax::{Highlight, Language, SyntaxTree};

use crate::{
    Theme,
    row_text::{column_at, offset_at, visible_text, word_at},
    text_input::{TextInput, TextInputEvent},
};

actions!(
    text_view,
    [
        LineDown,
        LineUp,
        PageDown,
        PageUp,
        ScrollLeft,
        ScrollRight,
        Top,
        Bottom,
        Copy,
        SelectAll,
        ClearSelection,
        Find,
        FindNext,
        FindPrevious,
        CloseFind
    ]
);

const KEY_CONTEXT: &str = "TextView";
const FIND_KEY_CONTEXT: &str = "FindBar";
/// Searches stop after this many matches.
const MAX_MATCHES: usize = 1_000_000;
/// Bytes loaded on each side of what's visible, so scrolling rarely waits on the disk.
const PREFETCH: u64 = 1024 * 1024;
const SCROLLBAR_WIDTH: f32 = 12.;
const MIN_THUMB_HEIGHT: f32 = 24.;
/// How often the view refreshes while its line index is being built.
const INDEX_REFRESH: Duration = Duration::from_millis(200);
/// Only this many bytes at the start of each line are highlighted (spec TXT-6).
const HIGHLIGHT_LINE_LIMIT: u64 = 10_000;
/// Larger selections aren't copied to the clipboard.
const MAX_COPY: u64 = 64_000_000;

/// Registers the read-mode key bindings (spec §4.2).
pub(crate) fn init(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("j", LineDown, context),
        KeyBinding::new("down", LineDown, context),
        KeyBinding::new("k", LineUp, context),
        KeyBinding::new("up", LineUp, context),
        KeyBinding::new("space", PageDown, context),
        KeyBinding::new("pagedown", PageDown, context),
        KeyBinding::new("shift-space", PageUp, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("left", ScrollLeft, context),
        KeyBinding::new("right", ScrollRight, context),
        KeyBinding::new("g", Top, context),
        KeyBinding::new("home", Top, context),
        KeyBinding::new("cmd-up", Top, context),
        KeyBinding::new("shift-g", Bottom, context),
        KeyBinding::new("end", Bottom, context),
        KeyBinding::new("cmd-down", Bottom, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("escape", ClearSelection, context),
        KeyBinding::new("cmd-f", Find, context),
        KeyBinding::new("/", Find, context),
        KeyBinding::new("n", FindNext, context),
        KeyBinding::new("shift-n", FindPrevious, context),
        KeyBinding::new("cmd-g", FindNext, context),
        KeyBinding::new("cmd-shift-g", FindPrevious, context),
    ]);
    let context = Some(FIND_KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("enter", FindNext, context),
        KeyBinding::new("shift-enter", FindPrevious, context),
        KeyBinding::new("cmd-g", FindNext, context),
        KeyBinding::new("cmd-shift-g", FindPrevious, context),
        KeyBinding::new("escape", CloseFind, context),
        KeyBinding::new("cmd-f", Find, context),
    ]);
}

/// How a text view draws text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    /// A multiple of the font size.
    pub line_height: f32,
    pub tab_size: usize,
}

/// What the status bar shows about a text view.
#[derive(Clone, Debug, PartialEq)]
pub struct TextViewStatus {
    /// The 1-based line at the top of the view, once that part is indexed.
    pub line: Option<u64>,
    /// The number of lines, estimated while indexing.
    pub lines: u64,
    /// How much has been indexed, while the line index is being built.
    pub indexing: Option<f32>,
    pub len: u64,
    pub language: Option<&'static str>,
    /// The number of bytes selected.
    pub selected: Option<u64>,
    pub error: Option<SharedString>,
    /// A passing message about the last action.
    pub notice: Option<SharedString>,
}

/// A read-only view of a text file of any size (spec TXT-1).
///
/// Only the visible rows are read, laid out and painted. The scroll position
/// is a byte offset rather than a line number, so the view can go anywhere in
/// a file whose line index is still being built. Data that isn't cached is
/// loaded in the background, never on the main thread.
pub struct TextView {
    source: Arc<FileSource>,
    index: Arc<LineIndex>,
    style: TextStyle,
    language: Option<&'static Language>,
    /// Set once the background parse finishes.
    syntax: Option<Arc<SyntaxTree>>,
    /// Stops the background parse when the view goes away.
    cancel_parse: Arc<AtomicBool>,
    focus_handle: FocusHandle,
    /// The row at the top of the viewport, as a byte offset. Always a row start.
    top: u64,
    /// How much of the top row is scrolled out of view, in `0..line_height`.
    top_offset: Pixels,
    scroll_x: Pixels,
    // Measurements from the last layout.
    line_height: Pixels,
    char_width: Pixels,
    max_scroll_x: Pixels,
    page_rows: usize,
    visible_len: u64,
    /// A 1-based line to scroll to once the index reaches it.
    pending_line: Option<u64>,
    pending_jump: Option<Jump>,
    loading: Option<Task<()>>,
    error: Option<SharedString>,
    notice: Option<SharedString>,
    drag: Option<ScrollbarDrag>,
    selection: Option<Selection>,
    /// Whether a mouse drag is extending the selection.
    selecting: bool,
    find: Option<FindState>,
    _index_refresh: Option<Task<()>>,
    _parse: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug)]
enum Jump {
    /// Bring the row containing this offset to the top.
    Offset(u64),
    /// Scroll the offset into view, a third of the way down, if it's not visible.
    Reveal(u64),
    /// Show the last page.
    End,
}

/// The find bar and its results (spec NAV-6).
struct FindState {
    input: Entity<TextInput>,
    /// Matches found so far, in order.
    matches: Vec<Range<u64>>,
    current: Option<usize>,
    searching: bool,
    /// Stops the running search.
    cancel: Arc<AtomicBool>,
    _search: Option<Task<()>>,
    _input_changes: Subscription,
}

/// Selected bytes, from where the selection started to where it ends now.
#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: u64,
    head: u64,
}

impl Selection {
    fn range(&self) -> Range<u64> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Clone, Copy)]
struct ScrollbarDrag {
    /// Where the thumb was grabbed, measured from its top.
    grab: Pixels,
    scrollbar: ScrollbarLayout,
}

impl TextView {
    /// A view of `source`, highlighted as `language` if it's in memory.
    pub fn new(
        source: Arc<FileSource>,
        index: Arc<LineIndex>,
        style: TextStyle,
        language: Option<&'static Language>,
        cx: &mut Context<Self>,
    ) -> Self {
        let index_refresh = (!index.is_complete()).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(INDEX_REFRESH).await;
                    let indexing = this.update(cx, |this, cx| {
                        cx.notify();
                        let progress = this.index.progress();
                        !progress.is_complete() && progress.failed.is_none()
                    });
                    if !matches!(indexing, Ok(true)) {
                        break;
                    }
                }
            })
        });
        let cancel_parse = Arc::new(AtomicBool::new(false));
        let parse = language
            .zip(source.in_memory().cloned())
            .map(|(language, text)| {
                let cancel = cancel_parse.clone();
                let parsing = cx.background_executor().spawn(async move {
                    SyntaxTree::parse(language, &text, || cancel.load(Ordering::Relaxed))
                });
                cx.spawn(async move |this, cx| {
                    if let Some(tree) = parsing.await {
                        this.update(cx, |this, cx| {
                            this.syntax = Some(Arc::new(tree));
                            cx.notify();
                        })
                        .ok();
                    }
                })
            });
        Self {
            source,
            index,
            style,
            language,
            syntax: None,
            cancel_parse,
            focus_handle: cx.focus_handle(),
            top: 0,
            top_offset: px(0.),
            scroll_x: px(0.),
            line_height: px(20.),
            char_width: px(8.),
            max_scroll_x: px(0.),
            page_rows: 1,
            visible_len: 0,
            pending_line: None,
            pending_jump: None,
            loading: None,
            error: None,
            notice: None,
            drag: None,
            selection: None,
            selecting: false,
            find: None,
            _index_refresh: index_refresh,
            _parse: parse,
        }
    }

    /// Another view of the same file at the same position, as for a split.
    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let source = self.source.clone();
        let index = self.index.clone();
        let style = self.style.clone();
        let syntax = self.syntax.clone();
        // Reuse the parse if it's done; otherwise parse again.
        let language = self.language.filter(|_| syntax.is_none());
        let top = self.top;
        cx.new(|cx| {
            let mut view = Self::new(source, index, style, language, cx);
            view.language = self.language;
            view.syntax = syntax;
            view.top = top;
            view
        })
    }

    pub fn status(&self) -> TextViewStatus {
        let progress = self.index.progress();
        let line = self.index.line_at(self.top, &*self.source).ok().flatten();
        TextViewStatus {
            line: line.map(|line| line + 1),
            lines: self
                .index
                .line_count()
                .unwrap_or_else(|| self.index.estimated_line_count()),
            indexing: (!progress.is_complete())
                .then(|| progress.indexed as f32 / progress.len.max(1) as f32),
            len: self.source.len(),
            language: self.language.map(|language| language.name),
            selected: self
                .selection
                .map(|selection| selection.range())
                .filter(|range| !range.is_empty())
                .map(|range| range.end - range.start),
            error: self.error.clone(),
            notice: self.notice.clone(),
        }
    }

    /// Scrolls so that 1-based `line` is at the top, waiting for the line
    /// index to reach it if needed.
    pub fn scroll_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        self.pending_line = Some(line);
        cx.notify();
    }

    fn apply_pending_line(&mut self, cx: &mut Context<Self>) {
        let Some(line) = self.pending_line else {
            return;
        };
        match self.index.line_start(line.saturating_sub(1), &*self.source) {
            Ok(Some(offset)) => {
                self.pending_line = None;
                self.pending_jump = Some(Jump::Offset(offset));
            }
            Ok(None) if self.index.is_complete() => {
                self.pending_line = None;
                self.pending_jump = Some(Jump::End);
            }
            Ok(None) => {}
            Err(err) => self.handle_read_error(err, cx),
        }
    }

    fn line_down(&mut self, _: &LineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.top_offset = px(0.);
        self.scroll_by(self.line_height, cx);
    }

    fn line_up(&mut self, _: &LineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.top_offset = px(0.);
        self.scroll_by(-self.line_height, cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.top_offset = px(0.);
        self.scroll_by(self.page_height(), cx);
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.top_offset = px(0.);
        self.scroll_by(-self.page_height(), cx);
    }

    fn scroll_left(&mut self, _: &ScrollLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_x_by(-self.char_width * 4., cx);
    }

    fn scroll_right(&mut self, _: &ScrollRight, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_x_by(self.char_width * 4., cx);
    }

    fn top(&mut self, _: &Top, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(Jump::Offset(0), cx);
    }

    fn bottom(&mut self, _: &Bottom, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(Jump::End, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some(range) = self
            .selection
            .map(|selection| selection.range())
            .filter(|range| !range.is_empty())
        else {
            return;
        };
        if range.end - range.start > MAX_COPY {
            self.notice = Some("The selection is too large to copy (over 64 MB).".into());
            cx.notify();
            return;
        }
        match self.source.bytes(range.clone()) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Err(ReadError::Missing(_)) => {
                // Copy once the selection is loaded, without moving the view.
                let source = self.source.clone();
                let text = cx.background_executor().spawn(async move {
                    source.load(range.clone())?;
                    let bytes = source
                        .bytes(range)
                        .map_err(|_| std::io::Error::other("the file changed"))?;
                    std::io::Result::Ok(String::from_utf8_lossy(&bytes).into_owned())
                });
                cx.spawn(async move |this, cx| {
                    let text = text.await;
                    this.update(cx, |this, cx| match text {
                        Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                        Err(err) => {
                            this.notice = Some(format!("Couldn't copy: {err}").into());
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            Err(err) => self.handle_read_error(err, cx),
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = Some(Selection {
            anchor: 0,
            head: self.source.len(),
        });
        cx.notify();
    }

    fn clear_selection(&mut self, _: &ClearSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = None;
        cx.notify();
    }

    fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        // Search for a short single-line selection, if there is one.
        let selected = self
            .selection
            .map(|selection| selection.range())
            .filter(|range| !range.is_empty() && range.end - range.start <= 256)
            .and_then(|range| self.source.bytes(range).ok())
            .filter(|bytes| !bytes.contains(&b'\n'))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let input = match &self.find {
            Some(find) => find.input.clone(),
            None => {
                let input = cx.new(|cx| TextInput::new("Find", cx));
                let input_changes = cx.subscribe(&input, |this, _, TextInputEvent::Changed, cx| {
                    this.search(cx)
                });
                self.find = Some(FindState {
                    input: input.clone(),
                    matches: Vec::new(),
                    current: None,
                    searching: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                    _search: None,
                    _input_changes: input_changes,
                });
                input
            }
        };
        input.update(cx, |input, cx| {
            if let Some(selected) = selected {
                input.set_text(selected, cx);
            }
            input.select_all_text(cx);
        });
        input.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn close_find(&mut self, _: &CloseFind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = self.find.take() {
            find.cancel.store(true, Ordering::Relaxed);
        }
        self.focus_handle.focus(window);
        cx.notify();
    }

    /// Starts searching for the find bar's text, replacing any running search.
    fn search(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else {
            return;
        };
        find.cancel.store(true, Ordering::Relaxed);
        find.matches.clear();
        find.current = None;
        find.searching = false;
        find._search = None;
        let Some(query) = Query::new(find.input.read(cx).text()) else {
            cx.notify();
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        find.cancel = cancel.clone();
        find.searching = true;
        let (batches, mut received) = mpsc::unbounded();
        let source = self.source.clone();
        let searching = cx.background_executor().spawn(async move {
            plastext_search::find_all(&source, &query, &cancel, |matches| {
                batches.unbounded_send(matches).ok();
            })
        });
        find._search = Some(cx.spawn(async move |this, cx| {
            while let Some(matches) = received.next().await {
                let added = this.update(cx, |this, cx| this.add_matches(matches, cx));
                if added.is_err() {
                    return;
                }
            }
            let result = searching.await;
            this.update(cx, |this, cx| {
                if let Some(find) = &mut this.find {
                    find.searching = false;
                }
                if let Err(err) = result {
                    this.notice = Some(format!("Search failed: {err}").into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn add_matches(&mut self, matches: Vec<Range<u64>>, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else {
            return;
        };
        find.matches.extend(matches);
        if find.matches.len() >= MAX_MATCHES {
            find.matches.truncate(MAX_MATCHES);
            find.cancel.store(true, Ordering::Relaxed);
        }
        // Show the first match from the current position as soon as there is one.
        if find.current.is_none() {
            let top = self.top;
            let next = find.matches.partition_point(|found| found.start < top);
            if next < find.matches.len() {
                self.select_match(next, cx);
            }
        }
        cx.notify();
    }

    fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(true, cx);
    }

    fn find_previous(&mut self, _: &FindPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(false, cx);
    }

    fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(find) = &self.find else {
            return;
        };
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        let index = match find.current {
            Some(current) if forward => (current + 1) % count,
            Some(current) => (current + count - 1) % count,
            None => {
                let next = find.matches.partition_point(|found| found.start < self.top);
                if forward {
                    next % count
                } else {
                    (next + count - 1) % count
                }
            }
        };
        self.select_match(index, cx);
    }

    /// Makes match `index` current: selected and scrolled into view.
    fn select_match(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else {
            return;
        };
        let Some(found) = find.matches.get(index).cloned() else {
            return;
        };
        find.current = Some(index);
        self.selection = Some(Selection {
            anchor: found.start,
            head: found.end,
        });
        self.jump(Jump::Reveal(found.start), cx);
    }

    fn render_find_bar(
        &self,
        find: &FindState,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = find.matches.len();
        let status = match (count, find.current, find.searching) {
            (0, _, true) => "Searching…".to_owned(),
            (0, _, false) if find.input.read(cx).text().is_empty() => String::new(),
            (0, _, false) => "No results".to_owned(),
            (_, current, searching) => {
                let more = if searching || count == MAX_MATCHES {
                    "+"
                } else {
                    ""
                };
                match current {
                    Some(current) => format!("{} of {count}{more}", current + 1),
                    None => format!("{count}{more} matches"),
                }
            }
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_color(theme.text_muted)
                .hover(|button| button.bg(theme.border))
                .child(label)
        };
        div()
            .key_context(FIND_KEY_CONTEXT)
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_action(cx.listener(Self::close_find))
            .h(px(34.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .child(div().flex_1().child(find.input.clone()))
            .child(div().text_xs().text_color(theme.text_muted).child(status))
            .child(
                button("find-previous", "↑")
                    .on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))),
            )
            .child(
                button("find-next", "↓")
                    .on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))),
            )
            .child(button("close-find", "×").on_click(
                cx.listener(|this, _, window, cx| this.close_find(&CloseFind, window, cx)),
            ))
            .into_any_element()
    }

    fn mouse_down(&mut self, hit: Hit, click_count: usize, extend: bool, cx: &mut Context<Self>) {
        self.notice = None;
        let row = hit.row.start..hit.row.end;
        let (selection, selecting) = match (hit.in_gutter, click_count) {
            (_, 1) if extend && self.selection.is_some() => {
                let anchor = self
                    .selection
                    .map_or(hit.offset, |selection| selection.anchor);
                (
                    Selection {
                        anchor,
                        head: hit.offset,
                    },
                    true,
                )
            }
            (true, _) | (false, 3..) => (
                Selection {
                    anchor: row.start,
                    head: row.end,
                },
                false,
            ),
            (false, 2) => {
                let word = word_at(&hit.row.content, (hit.offset - row.start) as usize);
                let word = row.start + word.start as u64..row.start + word.end as u64;
                (
                    Selection {
                        anchor: word.start,
                        head: word.end,
                    },
                    false,
                )
            }
            _ => (
                Selection {
                    anchor: hit.offset,
                    head: hit.offset,
                },
                true,
            ),
        };
        self.selection = Some(selection);
        self.selecting = selecting;
        cx.notify();
    }

    fn select_to(&mut self, offset: u64, cx: &mut Context<Self>) {
        if let Some(selection) = &mut self.selection
            && self.selecting
        {
            selection.head = offset;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        if self.selecting {
            self.selecting = false;
            if self
                .selection
                .is_some_and(|selection| selection.range().is_empty())
            {
                self.selection = None;
            }
            cx.notify();
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(self.line_height);
        self.scroll_x_by(-delta.x, cx);
        self.scroll_by(-delta.y, cx);
    }

    fn page_height(&self) -> Pixels {
        self.line_height * self.page_rows.saturating_sub(1).max(1) as f32
    }

    fn scroll_x_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        self.scroll_x = (self.scroll_x + delta).min(self.max_scroll_x).max(px(0.));
        cx.notify();
    }

    /// Scrolls down by `delta` (up if negative), a row at a time.
    fn scroll_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        self.pending_line = None;
        self.pending_jump = None;
        let mut offset = self.top_offset + delta;
        while offset >= self.line_height {
            match rows::row_end(&*self.source, self.top) {
                Ok(end) if end < self.source.len() => {
                    self.top = end;
                    offset -= self.line_height;
                }
                Ok(_) => offset = px(0.),
                Err(err) => {
                    self.handle_read_error(err, cx);
                    offset = px(0.);
                }
            }
        }
        while offset < px(0.) {
            match rows::previous_row_start(&*self.source, self.top) {
                Ok(Some(start)) => {
                    self.top = start;
                    offset += self.line_height;
                }
                Ok(None) => offset = px(0.),
                Err(err) => {
                    self.handle_read_error(err, cx);
                    offset = px(0.);
                }
            }
        }
        self.top_offset = offset;
        cx.notify();
    }

    fn jump(&mut self, jump: Jump, cx: &mut Context<Self>) {
        self.pending_jump = Some(jump);
        self.apply_pending_jump(cx);
        cx.notify();
    }

    /// Moves to the pending jump's target, or loads what's needed to find it.
    fn apply_pending_jump(&mut self, cx: &mut Context<Self>) {
        let Some(jump) = self.pending_jump else {
            return;
        };
        match self.jump_target(jump) {
            Ok(top) => {
                self.top = top;
                self.top_offset = px(0.);
                self.pending_jump = None;
            }
            Err(err) => {
                if err == ReadError::Changed {
                    self.pending_jump = None;
                }
                self.handle_read_error(err, cx);
            }
        }
    }

    fn jump_target(&self, jump: Jump) -> Result<u64, ReadError> {
        match jump {
            Jump::Offset(offset) => rows::row_start_at(&*self.source, offset),
            Jump::Reveal(offset) => {
                if (self.top..self.top + self.visible_len.saturating_sub(1)).contains(&offset) {
                    return Ok(self.top);
                }
                let mut top = rows::row_start_at(&*self.source, offset)?;
                for _ in 0..self.page_rows / 3 {
                    match rows::previous_row_start(&*self.source, top)? {
                        Some(start) => top = start,
                        None => break,
                    }
                }
                Ok(top)
            }
            Jump::End => {
                let last = self.source.len().saturating_sub(1);
                let mut top = rows::row_start_at(&*self.source, last)?;
                for _ in 1..self.page_rows {
                    match rows::previous_row_start(&*self.source, top)? {
                        Some(start) => top = start,
                        None => break,
                    }
                }
                Ok(top)
            }
        }
    }

    fn handle_read_error(&mut self, err: ReadError, cx: &mut Context<Self>) {
        match err {
            ReadError::Missing(range) => self.load(range, cx),
            ReadError::Changed => {
                self.error =
                    Some("This file changed on disk. Reopen it to see the new contents.".into());
            }
        }
    }

    /// Loads `range` and its surroundings in the background, one load at a time.
    fn load(&mut self, range: Range<u64>, cx: &mut Context<Self>) {
        if self.loading.is_some() || self.error.is_some() {
            return;
        }
        let source = self.source.clone();
        let range = range.start.saturating_sub(PREFETCH)..range.end.saturating_add(PREFETCH);
        let load = cx
            .background_executor()
            .spawn(async move { source.load(range) });
        self.loading = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.loading = None;
                if let Err(err) = result {
                    this.error = Some(format!("Couldn't read this file: {err}").into());
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn start_drag(&mut self, y: Pixels, scrollbar: ScrollbarLayout, cx: &mut Context<Self>) {
        let thumb = scrollbar.thumb;
        let grab = if (thumb.top()..thumb.bottom()).contains(&y) {
            y - thumb.top()
        } else {
            thumb.size.height / 2.
        };
        self.drag = Some(ScrollbarDrag { grab, scrollbar });
        self.drag_to(y, cx);
    }

    fn drag_to(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        let track = drag.scrollbar.track;
        let travel = track.size.height - drag.scrollbar.thumb.size.height;
        let fraction = if travel > px(0.) {
            ((y - drag.grab - track.top()) / travel).clamp(0., 1.)
        } else {
            0.
        };
        let scrollable = self.source.len().saturating_sub(self.visible_len);
        self.jump(
            Jump::Offset((fraction as f64 * scrollable as f64) as u64),
            cx,
        );
    }

    fn layout(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TextLayout {
        let theme = Theme::for_appearance(window.appearance());
        let text_system = window.text_system().clone();
        let font = font(self.style.font_family.clone());
        let font_size = self.style.font_size;
        let font_id = text_system.resolve_font(&font);
        let char_width = text_system.ch_advance(font_id, font_size).unwrap_or(px(8.));
        let line_height = (font_size * self.style.line_height).round();
        self.char_width = char_width;
        self.line_height = line_height;
        self.page_rows = ((bounds.size.height / line_height) as usize).max(1);
        self.apply_pending_line(cx);
        self.apply_pending_jump(cx);

        let lines = self
            .index
            .line_count()
            .unwrap_or_else(|| self.index.estimated_line_count());
        let digits = lines.max(1).ilog10() as usize + 1;
        let gutter_width = char_width * (digits.max(3) + 3) as f32;
        let text_left = gutter_width + char_width;
        let text_width = (bounds.size.width - text_left - px(SCROLLBAR_WIDTH)).max(px(0.));
        let first_column = (self.scroll_x / char_width) as usize;
        let shift = char_width * first_column as f32 - self.scroll_x;
        let columns = (text_width / char_width).ceil() as usize + 2;
        let shaper = Shaper {
            text_system: &text_system,
            font,
            font_size,
        };

        let row_count = (bounds.size.height / line_height).ceil() as usize + 1;
        let (ranges, err) = rows::rows(&*self.source, self.top, row_count);
        if let Some(err) = err {
            self.handle_read_error(err, cx);
        }
        let mut visible_rows = Vec::with_capacity(ranges.len());
        let mut starts_line = rows::starts_line(&*self.source, self.top).unwrap_or(false);
        for range in &ranges {
            match self.source.bytes(range.clone()) {
                Ok(bytes) => {
                    let ends_line = bytes.last() == Some(&b'\n');
                    visible_rows.push((range.start, bytes, starts_line));
                    starts_line = ends_line;
                }
                Err(err) => {
                    self.handle_read_error(err, cx);
                    break;
                }
            }
        }
        let spans = self.highlights(&visible_rows);

        let selection = self
            .selection
            .map(|selection| selection.range())
            .filter(|range| !range.is_empty());
        let mut line = self.index.line_at(self.top, &*self.source).ok().flatten();
        let mut rows = Vec::with_capacity(visible_rows.len());
        let mut hit_rows = Vec::with_capacity(visible_rows.len());
        let mut max_columns = 0;
        for (i, (start, bytes, starts_line)) in visible_rows.iter().enumerate() {
            if i > 0 && *starts_line {
                line = line.map(|line| line + 1);
            }
            let number = line
                .filter(|_| *starts_line)
                .map(|line| shaper.shape((line + 1).to_string(), &[(0, theme.text_muted)]));
            let content = bytes.strip_suffix(b"\n").unwrap_or(bytes);
            let content = content.strip_suffix(b"\r").unwrap_or(content);
            let row_spans = spans_within(&spans, *start..start + content.len() as u64);
            let visible = visible_text(
                content,
                self.style.tab_size,
                first_column..first_column + columns,
                &row_spans,
            );
            max_columns = max_columns.max(visible.width);
            let end = start + bytes.len() as u64;
            let selected = selection
                .clone()
                .filter(|selection| selection.start < end && selection.end > *start)
                .map(|selection| {
                    let tab_size = self.style.tab_size;
                    let column = |offset: u64| {
                        let offset = (offset - start) as usize;
                        if offset > content.len() {
                            // The selection includes the line break.
                            visible.width + 1
                        } else {
                            column_at(content, tab_size, offset)
                        }
                    };
                    let x =
                        |column: usize| char_width * (column as f32 - first_column as f32) + shift;
                    x(column(selection.start.max(*start)))..x(column(selection.end.min(end)))
                });
            let matches = self
                .find
                .as_ref()
                .map(|find| {
                    let first = find.matches.partition_point(|found| found.end <= *start);
                    find.matches[first..]
                        .iter()
                        .take_while(|found| found.start < end)
                        .map(|found| {
                            let tab_size = self.style.tab_size;
                            let column = |offset: u64| {
                                column_at(
                                    content,
                                    tab_size,
                                    (offset.clamp(*start, end) - start) as usize,
                                )
                            };
                            let x = |column: usize| {
                                char_width * (column as f32 - first_column as f32) + shift
                            };
                            x(column(found.start))..x(column(found.end))
                        })
                        .collect()
                })
                .unwrap_or_default();
            hit_rows.push(HitRow {
                start: *start,
                end,
                content: bytes.slice(0..content.len()),
            });
            let runs: Vec<(usize, Rgba)> = visible
                .runs
                .iter()
                .map(|&(len, highlight)| (len, highlight.map_or(theme.text, |h| theme.syntax(h))))
                .collect();
            rows.push(RowLayout {
                number,
                text: (!visible.text.is_empty()).then(|| shaper.shape(visible.text, &runs)),
                selected,
                matches,
            });
        }

        let shown_end = ranges.last().map_or(self.top, |range| range.end);
        self.visible_len = shown_end - self.top;
        self.max_scroll_x = (char_width * (max_columns + 2) as f32 - text_width).max(px(0.));
        self.scroll_x = self.scroll_x.min(self.max_scroll_x);

        let len = self.source.len();
        let fits = self.top == 0
            && shown_end == len
            && line_height * ranges.len() as f32 <= bounds.size.height;
        let scrollbar = (!fits && len > 0).then(|| {
            let track = Bounds::new(
                point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.top()),
                size(px(SCROLLBAR_WIDTH), bounds.size.height),
            );
            let height = bounds.size.height;
            let thumb_height = (height * (self.visible_len as f32 / len as f32))
                .max(px(MIN_THUMB_HEIGHT))
                .min(height);
            let scrollable = len.saturating_sub(self.visible_len).max(1);
            let fraction = (self.top as f32 / scrollable as f32).min(1.);
            let thumb = Bounds::new(
                point(
                    track.left() + px(3.),
                    track.top() + (height - thumb_height) * fraction,
                ),
                size(px(SCROLLBAR_WIDTH - 6.), thumb_height),
            );
            ScrollbarLayout { track, thumb }
        });

        if self.error.is_none() {
            let around = self.top.saturating_sub(PREFETCH)..shown_end.saturating_add(PREFETCH);
            if let Some(missing) = self.source.missing(around) {
                self.load(missing, cx);
            }
        }

        TextLayout {
            theme,
            line_height,
            top_offset: self.top_offset,
            gutter_width,
            text_left,
            shift,
            rows,
            hits: HitMap {
                bounds,
                gutter_width,
                text_left: text_left + shift,
                first_column,
                char_width,
                line_height,
                top_offset: self.top_offset,
                tab_size: self.style.tab_size,
                rows: hit_rows,
            },
            scrollbar,
            message: self
                .error
                .clone()
                .map(|error| shaper.shape(error.to_string(), &[(0, theme.text_muted)])),
        }
    }

    /// Highlighted spans for the visible rows, as absolute offsets: the start
    /// of each line, up to [`HIGHLIGHT_LINE_LIMIT`] bytes of it.
    fn highlights(&self, rows: &[(u64, Bytes, bool)]) -> Vec<(Range<u64>, Highlight)> {
        let (Some(syntax), Some(text)) = (&self.syntax, self.source.in_memory()) else {
            return Vec::new();
        };
        let mut ranges: Vec<Range<u64>> = Vec::new();
        for (start, bytes, starts_line) in rows {
            if !starts_line {
                continue;
            }
            let end = start + (bytes.len() as u64).min(HIGHLIGHT_LINE_LIMIT);
            match ranges.last_mut() {
                Some(last) if last.end == *start => last.end = end,
                _ => ranges.push(*start..end),
            }
        }
        ranges
            .into_iter()
            .flat_map(|range| syntax.highlights(text, range.start as usize..range.end as usize))
            .map(|(span, highlight)| (span.start as u64..span.end as u64, highlight))
            .collect()
    }
}

impl Drop for TextView {
    fn drop(&mut self) {
        self.cancel_parse.store(true, Ordering::Relaxed);
    }
}

/// The spans overlapping `row`, relative to its start.
fn spans_within(
    spans: &[(Range<u64>, Highlight)],
    row: Range<u64>,
) -> Vec<(Range<usize>, Highlight)> {
    let first = spans.partition_point(|(span, _)| span.end <= row.start);
    spans[first..]
        .iter()
        .take_while(|(span, _)| span.start < row.end)
        .map(|(span, highlight)| {
            let start = span.start.max(row.start) - row.start;
            let end = span.end.min(row.end) - row.start;
            (start as usize..end as usize, *highlight)
        })
        .collect()
}

impl Render for TextView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = Theme::for_appearance(window.appearance());
        // The find bar sits outside the view's key context, so typing in it
        // doesn't trigger single-key bindings such as `n` or `/`.
        let find_bar = self
            .find
            .as_ref()
            .map(|find| self.render_find_bar(find, theme, cx));
        let text = div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .flex_1()
            .min_h_0()
            .on_action(cx.listener(Self::line_down))
            .on_action(cx.listener(Self::line_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::scroll_left))
            .on_action(cx.listener(Self::scroll_right))
            .on_action(cx.listener(Self::top))
            .on_action(cx.listener(Self::bottom))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_selection))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus_handle.focus(window)),
            )
            .child(
                canvas(
                    {
                        let view = view.clone();
                        move |bounds, window, cx| {
                            view.update(cx, |view, cx| view.layout(bounds, window, cx))
                        }
                    },
                    move |bounds, layout, window, cx| layout.paint(bounds, view, window, cx),
                )
                .size_full(),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(find_bar)
            .child(text)
    }
}

impl Focusable for TextView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub(crate) struct Shaper<'a> {
    pub text_system: &'a WindowTextSystem,
    pub font: Font,
    pub font_size: Pixels,
}

impl Shaper<'_> {
    /// Shapes `text` colored by `runs` of byte lengths; a zero length means the rest.
    pub fn shape(&self, text: String, runs: &[(usize, Rgba)]) -> ShapedLine {
        let mut remaining = text.len();
        let runs: Vec<TextRun> = runs
            .iter()
            .map(|&(len, color)| {
                let len = if len == 0 {
                    remaining
                } else {
                    len.min(remaining)
                };
                remaining -= len;
                TextRun {
                    len,
                    font: self.font.clone(),
                    color: color.into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }
            })
            .collect();
        self.text_system
            .shape_line(text.into(), self.font_size, &runs, None)
    }
}

struct TextLayout {
    theme: Theme,
    line_height: Pixels,
    top_offset: Pixels,
    gutter_width: Pixels,
    text_left: Pixels,
    /// Horizontal offset of each row's text: the scrolled-off part of its first column.
    shift: Pixels,
    rows: Vec<RowLayout>,
    hits: HitMap,
    scrollbar: Option<ScrollbarLayout>,
    message: Option<ShapedLine>,
}

struct RowLayout {
    number: Option<ShapedLine>,
    text: Option<ShapedLine>,
    /// The selected part, as x offsets from the text's left edge.
    selected: Option<Range<Pixels>>,
    /// Find matches, likewise.
    matches: Vec<Range<Pixels>>,
}

/// Where the visible rows are, for turning mouse positions into offsets.
#[derive(Clone)]
struct HitMap {
    bounds: Bounds<Pixels>,
    gutter_width: Pixels,
    /// Where `first_column` starts, relative to `bounds`.
    text_left: Pixels,
    first_column: usize,
    char_width: Pixels,
    line_height: Pixels,
    top_offset: Pixels,
    tab_size: usize,
    rows: Vec<HitRow>,
}

#[derive(Clone)]
struct HitRow {
    start: u64,
    /// Past the row's line break, if it has one.
    end: u64,
    /// The row without its line break.
    content: Bytes,
}

/// A mouse position resolved to a byte offset.
struct Hit {
    offset: u64,
    row: HitRow,
    in_gutter: bool,
}

impl HitMap {
    /// The offset nearest `position`, clamped to the visible rows.
    fn hit(&self, position: gpui::Point<Pixels>) -> Option<Hit> {
        let last = self.rows.len().checked_sub(1)?;
        let y = position.y - self.bounds.top() + self.top_offset;
        let index = ((y / self.line_height).floor().max(0.) as usize).min(last);
        let row = self.rows[index].clone();
        let offset = if y < px(0.) {
            row.start
        } else if index == last && y >= self.line_height * self.rows.len() as f32 {
            row.end
        } else {
            let x = position.x - self.bounds.left() - self.text_left;
            let column =
                (self.first_column as f32 + (x / self.char_width).round()).max(0.) as usize;
            row.start + offset_at(&row.content, self.tab_size, column) as u64
        };
        Some(Hit {
            offset,
            in_gutter: position.x < self.bounds.left() + self.gutter_width,
            row,
        })
    }
}

#[derive(Clone, Copy)]
struct ScrollbarLayout {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
}

impl TextLayout {
    fn paint(
        self,
        bounds: Bounds<Pixels>,
        view: Entity<TextView>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = self.theme;
        let line_height = self.line_height;
        window.paint_quad(fill(bounds, theme.background));
        let gutter = Bounds::new(bounds.origin, size(self.gutter_width, bounds.size.height));
        window.paint_quad(fill(gutter, theme.surface));

        let row_top = |i: usize| bounds.top() - self.top_offset + line_height * i as f32;
        window.with_content_mask(Some(ContentMask { bounds: gutter }), |window| {
            for (i, row) in self.rows.iter().enumerate() {
                if let Some(number) = &row.number {
                    let x = gutter.right() - number.width - line_height / 2.;
                    number
                        .paint(point(x, row_top(i)), line_height, window, cx)
                        .ok();
                }
            }
        });
        let text_area =
            Bounds::from_corners(point(gutter.right(), bounds.top()), bounds.bottom_right());
        window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
            let x = bounds.left() + self.text_left + self.shift;
            for (i, row) in self.rows.iter().enumerate() {
                let highlights = row
                    .matches
                    .iter()
                    .map(|range| (range, theme.find_match))
                    .chain(row.selected.iter().map(|range| (range, theme.selection)));
                for (range, color) in highlights {
                    let left = bounds.left() + self.text_left + range.start;
                    let width = range.end - range.start;
                    let quad = Bounds::new(point(left, row_top(i)), size(width, line_height));
                    window.paint_quad(fill(quad, color));
                }
            }
            for (i, row) in self.rows.iter().enumerate() {
                if let Some(text) = &row.text {
                    text.paint(point(x, row_top(i)), line_height, window, cx)
                        .ok();
                }
            }
            if let Some(message) = &self.message {
                let y = bounds.bottom() - line_height * 1.5;
                message
                    .paint(
                        point(bounds.left() + self.text_left, y),
                        line_height,
                        window,
                        cx,
                    )
                    .ok();
            }
        });

        if let Some(scrollbar) = self.scrollbar {
            window.paint_quad(fill(scrollbar.thumb, theme.scrollbar_thumb).corner_radii(px(3.)));
            let view = view.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble
                    && event.button == MouseButton::Left
                    && scrollbar.track.contains(&event.position)
                {
                    view.update(cx, |view, cx| {
                        view.start_drag(event.position.y, scrollbar, cx)
                    });
                    cx.stop_propagation();
                }
            });
        }
        let text_bounds = Bounds::from_corners(
            bounds.origin,
            point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.bottom()),
        );
        let hits = self.hits.clone();
        let pressed = view.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && text_bounds.contains(&event.position)
                && let Some(hit) = hits.hit(event.position)
            {
                let extend = event.modifiers.shift;
                pressed.update(cx, |view, cx| {
                    view.mouse_down(hit, event.click_count, extend, cx)
                });
            }
        });
        let hits = self.hits;
        let dragged = view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || !event.dragging() {
                return;
            }
            dragged.update(cx, |view, cx| {
                if view.drag.is_some() {
                    view.drag_to(event.position.y, cx);
                } else if view.selecting {
                    // Dragging past an edge scrolls a row at a time.
                    if event.position.y < bounds.top() {
                        view.scroll_by(-view.line_height, cx);
                    } else if event.position.y > bounds.bottom() {
                        view.scroll_by(view.line_height, cx);
                    }
                    if let Some(hit) = hits.hit(event.position) {
                        view.select_to(hit.offset, cx);
                    }
                }
            });
        });
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                view.update(cx, |view, cx| view.mouse_up(cx));
            }
        });
    }
}
