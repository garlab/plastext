use std::{fmt::Write as _, ops::Range, sync::Arc};

use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent, ShapedLine,
    SharedString, Task, Window, canvas, div, fill, font, point, prelude::*, px, size,
};
use plastext_buffer::{ByteSource, FileSource, ReadError};

use crate::{
    TextStyle, Theme,
    text_view::{Bottom, LineDown, LineUp, PageDown, PageUp, Shaper, Top},
};

const KEY_CONTEXT: &str = "HexView";
const ROW_BYTES: u64 = 16;
/// Bytes loaded on each side of what's visible.
const PREFETCH: u64 = 1024 * 1024;
const SCROLLBAR_WIDTH: f32 = 12.;
const MIN_THUMB_HEIGHT: f32 = 24.;

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
        KeyBinding::new("g", Top, context),
        KeyBinding::new("home", Top, context),
        KeyBinding::new("cmd-up", Top, context),
        KeyBinding::new("shift-g", Bottom, context),
        KeyBinding::new("end", Bottom, context),
        KeyBinding::new("cmd-down", Bottom, context),
    ]);
}

/// What the status bar shows about a hex view.
#[derive(Clone, Debug, PartialEq)]
pub struct HexViewStatus {
    /// The offset of the top row.
    pub offset: u64,
    pub len: u64,
    pub error: Option<SharedString>,
}

/// A read-only hex dump of a file of any size (spec AST-4): offsets, 16
/// bytes per row in hex, and the same bytes as ASCII. Like the text view, it
/// lays out only the visible rows and loads data in the background.
pub struct HexView {
    source: Arc<FileSource>,
    style: TextStyle,
    focus_handle: FocusHandle,
    top_row: u64,
    /// How much of the top row is scrolled out of view, in `0..line_height`.
    top_offset: Pixels,
    line_height: Pixels,
    page_rows: u64,
    loading: Option<Task<()>>,
    error: Option<SharedString>,
    drag: Option<Drag>,
}

#[derive(Clone, Copy)]
struct Drag {
    grab: Pixels,
    scrollbar: Scrollbar,
}

#[derive(Clone, Copy)]
struct Scrollbar {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
}

impl HexView {
    pub fn new(source: Arc<FileSource>, style: TextStyle, cx: &mut Context<Self>) -> Self {
        Self {
            source,
            style,
            focus_handle: cx.focus_handle(),
            top_row: 0,
            top_offset: px(0.),
            line_height: px(20.),
            page_rows: 1,
            loading: None,
            error: None,
            drag: None,
        }
    }

    /// Another view of the same file at the same position, as for a split.
    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let source = self.source.clone();
        let style = self.style.clone();
        let top_row = self.top_row;
        cx.new(|cx| {
            let mut view = Self::new(source, style, cx);
            view.top_row = top_row;
            view
        })
    }

    pub fn status(&self) -> HexViewStatus {
        HexViewStatus {
            offset: self.top_row * ROW_BYTES,
            len: self.source.len(),
            error: self.error.clone(),
        }
    }

    fn last_row(&self) -> u64 {
        self.source.len().div_ceil(ROW_BYTES).saturating_sub(1)
    }

    fn scroll_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        if self.line_height <= px(0.) {
            return;
        }
        let offset = self.top_offset + delta;
        let rows = (offset / self.line_height).floor();
        let top = (self.top_row as f64 + rows as f64).clamp(0., self.last_row() as f64) as u64;
        self.top_offset = if top as f64 == self.top_row as f64 + rows as f64 {
            offset - self.line_height * rows
        } else {
            px(0.)
        };
        self.top_row = top;
        cx.notify();
    }

    fn scroll_to_row(&mut self, row: u64, cx: &mut Context<Self>) {
        self.top_row = row.min(self.last_row());
        self.top_offset = px(0.);
        cx.notify();
    }

    fn line_down(&mut self, _: &LineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_row(self.top_row + 1, cx);
    }

    fn line_up(&mut self, _: &LineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_row(self.top_row.saturating_sub(1), cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_row(self.top_row + self.page_rows.saturating_sub(1).max(1), cx);
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page_rows.saturating_sub(1).max(1);
        self.scroll_to_row(self.top_row.saturating_sub(page), cx);
    }

    fn top(&mut self, _: &Top, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_row(0, cx);
    }

    fn bottom(&mut self, _: &Bottom, _: &mut Window, cx: &mut Context<Self>) {
        let row = (self.last_row() + 1).saturating_sub(self.page_rows);
        self.scroll_to_row(row, cx);
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scroll_by(-event.delta.pixel_delta(self.line_height).y, cx);
    }

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
        let scrollable = (self.last_row() + 1).saturating_sub(self.page_rows);
        self.scroll_to_row((fraction as f64 * scrollable as f64) as u64, cx);
    }

    fn layout(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HexLayout {
        let theme = Theme::for_appearance(window.appearance());
        let text_system = window.text_system().clone();
        let shaper = Shaper {
            text_system: &text_system,
            font: font(self.style.font_family.clone()),
            font_size: self.style.font_size,
        };
        let line_height = (self.style.font_size * self.style.line_height).round();
        self.line_height = line_height;
        self.page_rows = ((bounds.size.height / line_height) as u64).max(1);

        let len = self.source.len();
        let row_count = (bounds.size.height / line_height).ceil() as u64 + 1;
        let first = self.top_row * ROW_BYTES;
        let end = ((self.top_row + row_count) * ROW_BYTES).min(len);
        let digits = (len.max(1).ilog2() / 4 + 1).max(8) as usize;
        let mut rows = Vec::new();
        match self.source.bytes(first.min(len)..end) {
            Ok(bytes) => {
                for (i, chunk) in bytes.chunks(ROW_BYTES as usize).enumerate() {
                    let (text, offset_len, hex_len) =
                        format_row(first + i as u64 * ROW_BYTES, chunk, digits);
                    let ascii_len = text.len() - offset_len - hex_len;
                    rows.push(shaper.shape(
                        text,
                        &[
                            (offset_len, theme.text_muted),
                            (hex_len, theme.text),
                            (ascii_len, theme.text_muted),
                        ],
                    ));
                }
            }
            Err(ReadError::Missing(range)) => self.load(range, cx),
            Err(ReadError::Changed) => {
                self.error =
                    Some("This file changed on disk. Reopen it to see the new contents.".into());
            }
        }
        if self.error.is_none()
            && let Some(missing) = self
                .source
                .missing(first.saturating_sub(PREFETCH)..end.saturating_add(PREFETCH))
        {
            self.load(missing, cx);
        }

        let total_rows = self.last_row() + 1;
        let scrollbar = (total_rows > self.page_rows).then(|| {
            let track = Bounds::new(
                point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.top()),
                size(px(SCROLLBAR_WIDTH), bounds.size.height),
            );
            let height = bounds.size.height;
            let thumb_height = (height * (self.page_rows as f32 / total_rows as f32))
                .max(px(MIN_THUMB_HEIGHT))
                .min(height);
            let scrollable = total_rows.saturating_sub(self.page_rows).max(1);
            let fraction = (self.top_row as f32 / scrollable as f32).min(1.);
            let thumb = Bounds::new(
                point(
                    track.left() + px(3.),
                    track.top() + (height - thumb_height) * fraction,
                ),
                size(px(SCROLLBAR_WIDTH - 6.), thumb_height),
            );
            Scrollbar { track, thumb }
        });

        HexLayout {
            theme,
            line_height,
            top_offset: self.top_offset,
            rows,
            scrollbar,
            message: self
                .error
                .clone()
                .map(|error| shaper.shape(error.to_string(), &[(0, theme.text_muted)])),
        }
    }
}

/// One row of the dump, plus the byte lengths of its offset and hex parts.
fn format_row(offset: u64, bytes: &[u8], digits: usize) -> (String, usize, usize) {
    let mut text = format!("{offset:0digits$x}  ");
    let offset_len = text.len();
    for i in 0..ROW_BYTES as usize {
        match bytes.get(i) {
            Some(byte) => write!(text, "{byte:02x} ").ok(),
            None => write!(text, "   ").ok(),
        };
        if i == 7 {
            text.push(' ');
        }
    }
    text.push(' ');
    let hex_len = text.len() - offset_len;
    text.extend(bytes.iter().map(|&byte| {
        if byte.is_ascii_graphic() || byte == b' ' {
            byte as char
        } else {
            '.'
        }
    }));
    (text, offset_len, hex_len)
}

impl Render for HexView {
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

impl Focusable for HexView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct HexLayout {
    theme: Theme,
    line_height: Pixels,
    top_offset: Pixels,
    rows: Vec<ShapedLine>,
    scrollbar: Option<Scrollbar>,
    message: Option<ShapedLine>,
}

impl HexLayout {
    fn paint(
        self,
        bounds: Bounds<Pixels>,
        view: Entity<HexView>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_quad(fill(bounds, self.theme.background));
        let left = bounds.left() + px(12.);
        for (i, row) in self.rows.iter().enumerate() {
            let y = bounds.top() - self.top_offset + self.line_height * i as f32;
            row.paint(point(left, y), self.line_height, window, cx).ok();
        }
        if let Some(message) = &self.message {
            let y = bounds.bottom() - self.line_height * 1.5;
            message
                .paint(point(left, y), self.line_height, window, cx)
                .ok();
        }
        if let Some(scrollbar) = self.scrollbar {
            window
                .paint_quad(fill(scrollbar.thumb, self.theme.scrollbar_thumb).corner_radii(px(3.)));
            let pressed = view.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble
                    && event.button == MouseButton::Left
                    && scrollbar.track.contains(&event.position)
                {
                    let thumb = scrollbar.thumb;
                    let y = event.position.y;
                    let grab = if (thumb.top()..thumb.bottom()).contains(&y) {
                        y - thumb.top()
                    } else {
                        thumb.size.height / 2.
                    };
                    pressed.update(cx, |view, cx| {
                        view.drag = Some(Drag { grab, scrollbar });
                        view.drag_to(y, cx);
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

#[cfg(test)]
mod tests {
    use super::format_row;

    #[test]
    fn formats_offsets_hex_and_ascii() {
        let (text, offset_len, hex_len) = format_row(0x20, b"Hello, world!\x00\x01\xff", 8);
        assert_eq!(
            text,
            "00000020  48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21 00 01 ff  Hello, world!..."
        );
        assert_eq!(&text[..offset_len], "00000020  ");
        assert_eq!(&text[offset_len + hex_len..], "Hello, world!...");

        let (text, _, _) = format_row(0x30, b"ab", 8);
        assert_eq!(
            text,
            // 14 missing bytes, the gap after the 8th byte, and the gap before the ASCII.
            format!("00000030  61 62 {}ab", " ".repeat(3 * 14 + 1 + 1))
        );
    }
}
