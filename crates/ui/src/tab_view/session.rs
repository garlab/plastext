use super::*;
use gpui::{IntoElement, Render, Window, div, prelude::*};
use plastext_settings::session::{PdfPosition, SessionScroll, SourcePosition, ViewMode, ViewState};

pub(crate) struct DeferredView {
    pub state: ViewState,
    pub error: Option<SharedString>,
    focus: FocusHandle,
}
impl DeferredView {
    pub fn new(state: ViewState, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            error: None,
            focus: cx.focus_handle(),
        }
    }
}
impl Focusable for DeferredView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for DeferredView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = plastext_editor::Theme::get(window, cx);
        div()
            .size_full()
            .p_4()
            .track_focus(&self.focus)
            .text_color(theme.text_muted)
            .child(self.error.clone().unwrap_or_else(|| "Opening file…".into()))
    }
}

fn source(p: plastext_editor::ScrollAnchor) -> SourcePosition {
    SourcePosition {
        start: p.start,
        end: p.end,
        fraction: p.fraction,
    }
}
fn anchor(p: SourcePosition) -> plastext_editor::ScrollAnchor {
    plastext_editor::ScrollAnchor::new(p.start, p.end, p.fraction)
}
fn saved(position: crate::scroll_sync::Position) -> SessionScroll {
    SessionScroll {
        source: source(position.source),
        rendered: position.rendered.map(|r| (r.block, source(r.span))),
    }
}
fn position(s: SessionScroll) -> crate::scroll_sync::Position {
    crate::scroll_sync::Position {
        source: anchor(s.source),
        rendered: s
            .rendered
            .map(|(block, span)| crate::scroll_sync::RenderedAnchor {
                block,
                span: anchor(span),
            }),
    }
}
pub(crate) fn pdf_state(state: (u32, bool, gpui::Point<f64>)) -> PdfPosition {
    PdfPosition {
        page: state.0,
        fit_page: state.1,
        pan: [state.2.x, state.2.y],
    }
}
pub(crate) fn pdf_reading(p: PdfPosition) -> (u32, bool, gpui::Point<f64>) {
    (p.page, p.fit_page, gpui::point(p.pan[0], p.pan[1]))
}
impl TabView {
    pub(crate) fn session_state(&self, cx: &App) -> ViewState {
        if let Self::Deferred(view) = self {
            return view.read(cx).state.clone();
        }
        let mode = match self {
            Self::Text(_) => ViewMode::Source,
            Self::Hex(_) => ViewMode::Binary,
            Self::Markdown(v) => {
                if v.read(cx).is_rendered() {
                    ViewMode::Rendered
                } else {
                    ViewMode::Source
                }
            }
            Self::Tex(v) => v.read(cx).session_mode(),
            Self::Pdf(v) => {
                if v.read(cx).is_binary() {
                    ViewMode::Binary
                } else {
                    ViewMode::Pdf
                }
            }
            Self::Image(v) => {
                if v.read(cx).is_raw() {
                    ViewMode::Source
                } else {
                    ViewMode::Rendered
                }
            }
            Self::Csv(v) => {
                if v.read(cx).is_raw() {
                    ViewMode::Source
                } else {
                    ViewMode::Rendered
                }
            }
            Self::Record(v) => {
                if v.read(cx).is_raw() {
                    ViewMode::Source
                } else {
                    ViewMode::Rendered
                }
            }
            Self::Deferred(_) => unreachable!(),
        };
        let pdf = match self {
            Self::Pdf(v) => Some(pdf_state(v.read(cx).reading_state())),
            Self::Tex(v) => v.read(cx).session_pdf(cx).map(pdf_state),
            _ => None,
        };
        let scroll = match self {
            Self::Markdown(v) => v.read(cx).session_scroll(cx),
            Self::Tex(v) if mode != ViewMode::Pdf => {
                v.read(cx).rendered_view().read(cx).session_scroll(cx)
            }
            _ => self.scroll_position(cx),
        };
        let offset = match self {
            Self::Pdf(v) => v.read(cx).session_offset(cx),
            Self::Image(v) => v.read(cx).session_offset(cx),
            Self::Markdown(v) => v.read(cx).session_offset(cx),
            Self::Tex(v) => v.read(cx).rendered_view().read(cx).session_offset(cx),
            _ => self.position(cx),
        };
        ViewState {
            mode,
            offset,
            scroll: scroll.map(saved),
            pdf,
        }
    }
    pub(crate) fn restore_session(&self, state: &ViewState, window: &mut Window, cx: &mut App) {
        let focus = window.focused(cx);
        let was_focused = self.focus_handle(cx).contains_focused(window, cx);
        match self {
            Self::Deferred(view) => view.update(cx, |v, _| v.state = state.clone()),
            Self::Markdown(view) => view.update(cx, |v, cx| {
                v.set_rendered(state.mode != ViewMode::Source, window, cx)
            }),
            Self::Tex(view) => view.update(cx, |v, cx| v.restore_session_mode(state, window, cx)),
            Self::Pdf(view) => view.update(cx, |v, cx| v.restore_session_mode(state, window, cx)),
            Self::Image(view) => view.update(cx, |v, cx| v.restore_session_mode(state, window, cx)),
            Self::Csv(view) => view.update(cx, |v, cx| {
                v.set_raw(state.mode == ViewMode::Source, window, cx)
            }),
            Self::Record(view) => view.update(cx, |v, cx| {
                v.set_raw(state.mode == ViewMode::Source, window, cx)
            }),
            _ => {}
        }
        if !matches!(self, Self::Pdf(_) | Self::Image(_)) {
            self.scroll_to_offset(state.offset, cx);
        }
        if let Some(scroll) = state.scroll {
            self.sync_scroll(position(scroll), cx);
        }
        if was_focused {
            self.focus_handle(cx).focus(window);
        } else if let Some(focus) = focus {
            focus.focus(window);
        }
    }
}
