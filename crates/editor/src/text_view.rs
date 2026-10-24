use std::{borrow::Cow, ops::Range, sync::Arc, time::Duration};

use gpui::{
    App, Bounds, ContentMask, Context, DispatchPhase, Entity, FocusHandle, Focusable, Font,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, ShapedLine, SharedString, Task, TextRun, Window, WindowTextSystem, actions,
    canvas, div, fill, font, point, prelude::*, px, size,
};
use plastext_buffer::{ByteSource, FileSource, LineIndex, ReadError, rows};

use crate::Theme;

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
        Bottom
    ]
);

const KEY_CONTEXT: &str = "TextView";
const FONT_FAMILY: &str = "Menlo";
const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f32 = 1.5;
const TAB_WIDTH: usize = 4;
/// Bytes loaded on each side of what's visible, so scrolling rarely waits on the disk.
const PREFETCH: u64 = 1024 * 1024;
const SCROLLBAR_WIDTH: f32 = 12.;
const MIN_THUMB_HEIGHT: f32 = 24.;
/// How often the view refreshes while its line index is being built.
const INDEX_REFRESH: Duration = Duration::from_millis(200);

/// Registers the read-mode key bindings (spec §4.2).
pub fn init(cx: &mut App) {
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
    ]);
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
    pub error: Option<SharedString>,
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
    drag: Option<ScrollbarDrag>,
    _index_refresh: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug)]
enum Jump {
    /// Bring the row containing this offset to the top.
    Offset(u64),
    /// Show the last page.
    End,
}

#[derive(Clone, Copy)]
struct ScrollbarDrag {
    /// Where the thumb was grabbed, measured from its top.
    grab: Pixels,
    scrollbar: ScrollbarLayout,
}

impl TextView {
    pub fn new(source: Arc<FileSource>, index: Arc<LineIndex>, cx: &mut Context<Self>) -> Self {
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
        Self {
            source,
            index,
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
            drag: None,
            _index_refresh: index_refresh,
        }
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
            error: self.error.clone(),
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
        let font = font(FONT_FAMILY);
        let font_size = px(FONT_SIZE);
        let font_id = text_system.resolve_font(&font);
        let char_width = text_system.ch_advance(font_id, font_size).unwrap_or(px(8.));
        let line_height = (font_size * LINE_HEIGHT).round();
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
        let mut line = self.index.line_at(self.top, &*self.source).ok().flatten();
        let mut starts_line = rows::starts_line(&*self.source, self.top).unwrap_or(false);
        let mut rows = Vec::with_capacity(ranges.len());
        let mut max_columns = 0;
        for (i, range) in ranges.iter().enumerate() {
            let bytes = match self.source.bytes(range.clone()) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.handle_read_error(err, cx);
                    break;
                }
            };
            if i > 0 && starts_line {
                line = line.map(|line| line + 1);
            }
            let number = line
                .filter(|_| starts_line)
                .map(|line| shaper.shape((line + 1).to_string(), theme.text_muted));
            let content = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
            starts_line = content.len() < bytes.len();
            let content = content.strip_suffix(b"\r").unwrap_or(content);
            let (text, width) = visible_text(content, first_column, columns);
            max_columns = max_columns.max(width);
            rows.push(RowLayout {
                number,
                text: (!text.is_empty()).then(|| shaper.shape(text, theme.text)),
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
            scrollbar,
            message: self
                .error
                .clone()
                .map(|error| shaper.shape(error.to_string(), theme.text_muted)),
        }
    }
}

impl Render for TextView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(Self::line_down))
            .on_action(cx.listener(Self::line_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::scroll_left))
            .on_action(cx.listener(Self::scroll_right))
            .on_action(cx.listener(Self::top))
            .on_action(cx.listener(Self::bottom))
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
            )
    }
}

impl Focusable for TextView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct Shaper<'a> {
    text_system: &'a WindowTextSystem,
    font: Font,
    font_size: Pixels,
}

impl Shaper<'_> {
    fn shape(&self, text: String, color: impl Into<gpui::Hsla>) -> ShapedLine {
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color: color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.text_system
            .shape_line(text.into(), self.font_size, &[run], None)
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
    scrollbar: Option<ScrollbarLayout>,
    message: Option<ShapedLine>,
}

struct RowLayout {
    number: Option<ShapedLine>,
    text: Option<ShapedLine>,
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
        let dragged = view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.dragging() {
                dragged.update(cx, |view, cx| view.drag_to(event.position.y, cx));
            }
        });
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                view.update(cx, |view, _| view.drag = None);
            }
        });
    }
}

/// Columns `first..first + count` of a row's text, with tabs expanded and
/// control characters made visible, plus the row's full width in columns.
///
/// Each character counts as one column, which is exact for the monospace
/// text this view targets except for wide (e.g. CJK) characters.
fn visible_text(bytes: &[u8], first: usize, count: usize) -> (String, usize) {
    let text: Cow<str> = String::from_utf8_lossy(bytes);
    let end = first + count;
    let mut visible = String::new();
    let mut column = 0;
    for ch in text.chars() {
        let (shown, width) = match ch {
            '\t' => (' ', TAB_WIDTH - column % TAB_WIDTH),
            '\u{7f}' => ('\u{2421}', 1),
            ch if (ch as u32) < 0x20 => {
                (char::from_u32(0x2400 + ch as u32).unwrap_or('\u{fffd}'), 1)
            }
            ch if ch.is_control() => ('\u{fffd}', 1),
            ch => (ch, 1),
        };
        for col in column..column + width {
            if (first..end).contains(&col) {
                visible.push(shown);
            }
        }
        column += width;
    }
    (visible, column)
}

#[cfg(test)]
mod tests {
    use super::visible_text;

    #[test]
    fn expands_tabs_and_shows_control_characters() {
        assert_eq!(visible_text(b"a\tb", 0, 80), ("a   b".to_owned(), 5));
        assert_eq!(
            visible_text(b"\x00x\x7f", 0, 80),
            ("\u{2400}x\u{2421}".to_owned(), 3)
        );
        assert_eq!(visible_text("é\u{fffd}".as_bytes(), 0, 80).1, 2);
        assert_eq!(visible_text(b"\xffz", 0, 80), ("\u{fffd}z".to_owned(), 2));
    }

    #[test]
    fn keeps_only_the_visible_columns() {
        assert_eq!(visible_text(b"0123456789", 3, 4), ("3456".to_owned(), 10));
        assert_eq!(visible_text(b"\tab", 2, 3), ("  a".to_owned(), 6));
        assert_eq!(visible_text(b"short", 10, 5), (String::new(), 5));
    }
}
