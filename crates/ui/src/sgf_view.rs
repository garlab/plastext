mod model;
mod viewport;

use document_graphics::{
    RasterOptions,
    sgf::{self, Collection, Color, NodeId, Position, RenderOptions},
};
use gpui::{
    AnyElement, App, ContentMask, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    Pixels, RenderImage, SharedString, Subscription, Task, Window, WindowAppearance, actions,
    canvas, div, prelude::*, px, relative,
};
use model::{Cursor, Positions};
use plastext_buffer::{ByteSource, FileSource, LineIndex};
use plastext_editor::{TextStyle, TextView, Theme};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

actions!(
    sgf_view,
    [
        ToggleSource,
        Next,
        Previous,
        First,
        Last,
        NextVariation,
        PreviousVariation
    ]
);
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-shift-v", ToggleSource, Some("Sgf")),
        KeyBinding::new("right", Next, Some("SgfPreview")),
        KeyBinding::new("left", Previous, Some("SgfPreview")),
        KeyBinding::new("home", First, Some("SgfPreview")),
        KeyBinding::new("end", Last, Some("SgfPreview")),
        KeyBinding::new("down", NextVariation, Some("SgfPreview")),
        KeyBinding::new("up", PreviousVariation, Some("SgfPreview")),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Request {
    node: NodeId,
    dark: bool,
    numbers: bool,
}
struct Pending {
    request: Option<Request>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct SgfView {
    source: Arc<FileSource>,
    lines: Arc<LineIndex>,
    style: TextStyle,
    zoom: f32,
    focus: FocusHandle,
    raw: Option<Entity<TextView>>,
    raw_observer: Option<Subscription>,
    show_raw: bool,
    visible: bool,
    cursor: Option<Cursor>,
    positions: Positions,
    presented: Option<Arc<Position>>,
    image: Option<Arc<RenderImage>>,
    rendered: Option<Request>,
    pending: Option<Pending>,
    pending_offset: u64,
    numbers: bool,
    error: Option<SharedString>,
}
impl SgfView {
    pub fn new(
        source: Arc<FileSource>,
        lines: Arc<LineIndex>,
        style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|this, cx| this.release_preview(cx)).detach();
        Self {
            source,
            lines,
            style,
            zoom: 1.,
            focus: cx.focus_handle(),
            raw: None,
            raw_observer: None,
            show_raw: false,
            visible: true,
            cursor: None,
            positions: Positions::default(),
            presented: None,
            image: None,
            rendered: None,
            pending: None,
            pending_offset: 0,
            numbers: false,
            error: None,
        }
    }
    fn request(&mut self, dark: bool, cx: &mut Context<Self>) {
        if !self.visible || self.show_raw {
            return;
        }
        let wanted = self.cursor.as_ref().map(|c| Request {
            node: c.selected,
            dark,
            numbers: self.numbers,
        });
        if let Some(pending) = &self.pending {
            if pending.request.is_some() && pending.request != wanted {
                pending.cancel.store(true, Ordering::Relaxed);
            }
            return; // Coalesce navigation: at most one running job, then the latest selection.
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let task = if let Some(request) = wanted {
            if self.rendered == wanted {
                return;
            }
            let doc = self.cursor.as_ref().unwrap().document.clone();
            let positions = self.positions.clone();
            let work = cx.background_executor().spawn(async move {
                let position = positions.get(&doc, request.node, &token)?;
                let raster = sgf::rasterize(
                    &position,
                    RenderOptions {
                        dark: request.dark,
                        move_numbers: request.numbers,
                    },
                    RasterOptions {
                        bounds: (1000, 1000),
                        scale: 4.,
                    },
                    &token,
                )?;
                let image = crate::image_view::render_image(plastext_assets::Preview {
                    width: raster.width,
                    height: raster.height,
                    original_width: raster.original_width,
                    original_height: raster.original_height,
                    rgba: raster.rgba,
                })
                .0;
                Ok::<_, sgf::Error>((position, image))
            });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |this, cx| {
                    let cancelled = this
                        .pending
                        .as_ref()
                        .is_none_or(|p| p.cancel.load(Ordering::Relaxed));
                    this.pending = None;
                    if !cancelled
                        && this.visible
                        && !this.show_raw
                        && this
                            .cursor
                            .as_ref()
                            .is_some_and(|c| c.selected == request.node)
                    {
                        this.rendered = Some(request);
                        match result {
                            Ok((position, image)) => {
                                this.positions.remember(position.clone());
                                this.presented = Some(position);
                                this.release_image(cx);
                                this.image = Some(image);
                                this.error = None;
                            }
                            Err(error) => {
                                this.error = Some(error.to_string().into());
                                this.release_image(cx);
                                this.presented = None;
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
        } else {
            if self.error.is_some() {
                return;
            }
            let source = self.source.clone();
            let work = cx.background_executor().spawn(async move {
                if source.len() > sgf::MAX_SOURCE as u64 {
                    return Err(
                        "SGF preview supports files up to 8 MiB. Open Source to read this file."
                            .to_owned(),
                    );
                }
                let bytes = source
                    .head(sgf::MAX_SOURCE as u64)
                    .map_err(|e| e.to_string())?;
                Collection::parse(Arc::from(bytes.as_ref()), &token)
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |this, cx| {
                    let cancelled = this
                        .pending
                        .as_ref()
                        .is_none_or(|p| p.cancel.load(Ordering::Relaxed));
                    this.pending = None;
                    if !cancelled {
                        match result {
                            Ok(document) => {
                                let mut cursor = Cursor::new(document);
                                cursor.select(
                                    cursor
                                        .document
                                        .node_for_offset(this.pending_offset as usize),
                                );
                                this.cursor = Some(cursor);
                            }
                            Err(error) => this.error = Some(error.into()),
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
        };
        self.pending = Some(Pending {
            request: wanted,
            cancel,
            _task: task,
        });
    }
    fn select(&mut self, node: Option<NodeId>, cx: &mut Context<Self>) {
        if let (Some(cursor), Some(node)) = (&mut self.cursor, node)
            && cursor.selected != node
        {
            cursor.select(node);
            self.error = None;
            if let Some(pending) = &self.pending {
                pending.cancel.store(true, Ordering::Relaxed);
            }
            cx.notify();
        }
    }
    fn navigate(&mut self, motion: Motion, window: &mut Window, cx: &mut Context<Self>) {
        let node = self.cursor.as_ref().and_then(|c| match motion {
            Motion::First => Some(c.first()),
            Motion::Previous => c.previous(),
            Motion::Next => c.next(),
            Motion::Last => Some(c.last()),
            Motion::Sibling(direction) => c.sibling(direction),
            Motion::Game(direction) => {
                let index = (c.game() as i32 + direction)
                    .rem_euclid(c.document.games().len() as i32)
                    as usize;
                Some(c.document.games()[index].root)
            }
        });
        self.select(node, cx);
        self.focus.focus(window);
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.release_preview(cx);
        }
        cx.notify();
    }
    fn release_image(&mut self, cx: &mut App) {
        if let Some(image) = self.image.take() {
            // GPUI's atlas outlives RenderImage; evict the old texture explicitly.
            cx.defer(move |cx| cx.drop_image(image, None));
        }
    }
    fn release_preview(&mut self, cx: &mut App) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.release_image(cx);
        self.rendered = None;
        self.positions = Positions::default();
        self.presented = None;
    }
    pub fn notice(&self) -> Option<SharedString> {
        self.error.clone()
    }
    pub fn summary(&self) -> String {
        if self.show_raw {
            return "Source    SGF".into();
        }
        self.presented.as_ref().map_or_else(
            || "SGF".into(),
            |p| {
                format!(
                    "{} × {}    Move {}    SGF",
                    p.width, p.height, p.move_number
                )
            },
        )
    }
    pub fn position(&self, cx: &App) -> u64 {
        if self.show_raw
            && let Some(raw) = &self.raw
        {
            return raw.read(cx).position();
        }
        self.cursor.as_ref().map_or(self.pending_offset, |c| {
            c.document.nodes()[c.selected].source.start as u64
        })
    }
    pub fn scroll_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        if let Ok(Some(offset)) = self.lines.line_start(line.saturating_sub(1), &*self.source) {
            self.scroll_to_offset(offset, cx);
        }
    }
    pub fn scroll_to_offset(&mut self, offset: u64, cx: &mut Context<Self>) {
        self.pending_offset = offset;
        if let Some(raw) = &self.raw {
            raw.update(cx, |v, cx| v.scroll_to_offset(offset, cx));
        }
        let node = self
            .cursor
            .as_ref()
            .map(|c| c.document.node_for_offset(offset as usize));
        self.select(node, cx);
    }
    pub fn set_zoom(&mut self, factor: f32, font_size: Pixels, cx: &mut Context<Self>) {
        self.zoom = factor;
        self.style.font_size = font_size;
        if let Some(raw) = &self.raw {
            raw.update(cx, |v, cx| v.set_font_size(font_size, cx));
        }
        cx.notify();
    }
    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let raw = self
            .raw
            .as_ref()
            .map(|v| v.update(cx, |v, cx| v.duplicate(cx)));
        cx.new(|cx| {
            let mut view = Self::new(
                self.source.clone(),
                self.lines.clone(),
                self.style.clone(),
                cx,
            );
            view.zoom = self.zoom;
            view.cursor = self.cursor.clone();
            view.positions = self.positions.clone();
            view.presented = self.presented.clone();
            // Each pane owns its raster/atlas entry; compact positions are shared.
            view.show_raw = self.show_raw;
            view.numbers = self.numbers;
            view.pending_offset = self.pending_offset;
            view.error = self.error.clone();
            view.raw = raw;
            if let Some(raw) = &view.raw {
                view.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
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
            if self.raw_observer.is_none() {
                self.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
            raw.update(cx, |v, cx| v.scroll_to_offset(offset, cx));
            raw.focus_handle(cx).focus(window);
            self.release_preview(cx);
        } else {
            if let Some(raw) = &self.raw {
                self.scroll_to_offset(raw.read(cx).position(), cx);
            }
            self.focus.focus(window);
        }
        self.show_raw = !self.show_raw;
        cx.notify();
    }
    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        motion: Motion,
        enabled: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .child(label)
            .text_color(if enabled {
                theme.text
            } else {
                theme.text_muted
            })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(theme.selection))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.navigate(motion, window, cx)),
                    )
            })
    }
    fn preview(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let cursor = self.cursor.as_ref();
        let ready = cursor.is_some();
        let mut controls = div()
            .id("sgf-controls")
            .flex_none()
            .max_h(relative(0.25))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_3()
            .py_1()
            .text_size(px(13. * self.zoom))
            .child(self.button(
                "sgf-first",
                "|‹",
                Motion::First,
                cursor.is_some_and(|c| c.previous().is_some()),
                theme,
                cx,
            ))
            .child(self.button(
                "sgf-previous",
                "‹",
                Motion::Previous,
                cursor.is_some_and(|c| c.previous().is_some()),
                theme,
                cx,
            ))
            .child(self.button(
                "sgf-next",
                "›",
                Motion::Next,
                cursor.is_some_and(|c| c.next().is_some()),
                theme,
                cx,
            ))
            .child(self.button(
                "sgf-last",
                "›|",
                Motion::Last,
                cursor.is_some_and(|c| c.next().is_some()),
                theme,
                cx,
            ));
        if let Some(p) = &self.presented {
            let last = p
                .last_move
                .map(|m| {
                    format!(
                        " · {} {}",
                        if m.color == Color::Black {
                            "Black"
                        } else {
                            "White"
                        },
                        m.point
                            .map_or_else(|| "passes".into(), |pnt| pnt.coordinate(p.height))
                    )
                })
                .unwrap_or_default();
            controls = controls.child(div().px_2().child(format!("Move {}{last}", p.move_number)));
        }
        controls = controls.child(
            div()
                .id("sgf-numbers")
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .when(self.numbers, |d| d.bg(theme.selection))
                .child("Numbers")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.numbers = !this.numbers;
                    this.error = None;
                    this.focus.focus(window);
                    cx.notify();
                })),
        );
        if let Some(c) = cursor
            && c.document.games().len() > 1
        {
            controls = controls
                .child(self.button(
                    "sgf-game-prev",
                    "Previous game",
                    Motion::Game(-1),
                    true,
                    theme,
                    cx,
                ))
                .child(format!("{}/{}", c.game() + 1, c.document.games().len()))
                .child(self.button(
                    "sgf-game-next",
                    "Next game",
                    Motion::Game(1),
                    true,
                    theme,
                    cx,
                ));
        }
        let mut details = div()
            .id("sgf-details")
            .flex_none()
            .max_h(relative(0.35))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .py_2()
            .text_size(px(13. * self.zoom));
        if let Some(c) = cursor {
            // Keep annotations paired with the displayed board while a new frame is pending.
            let node = self.presented.as_ref().map_or(c.selected, |p| p.node());
            let game = &c.document.games()[c.document.nodes()[node].game];
            let text = |key| clipped(c.document.text(game.root, key).unwrap_or_default(), 256);
            details = details.child(div().font_weight(gpui::FontWeight::BOLD).child(clipped(
                c.document.title(c.document.nodes()[node].game),
                256,
            )));
            let mut info = vec![];
            for (label, key) in [
                ("Black", "PB"),
                ("White", "PW"),
                ("Result", "RE"),
                ("Komi", "KM"),
                ("Date", "DT"),
            ] {
                let value = text(key);
                if !value.is_empty() {
                    info.push(format!("{label}: {value}"));
                }
            }
            if let Some(p) = &self.presented {
                info.push(format!(
                    "Captures: Black {} · White {}",
                    p.captures[0], p.captures[1]
                ));
            }
            details = details.child(div().text_color(theme.text_muted).child(info.join("    ")));
            let comment = c.document.text(node, "C").or_else(|| {
                if node == game.root {
                    c.document.text(node, "GC")
                } else {
                    None
                }
            });
            if let Some(comment) = comment.filter(|s| !s.is_empty()) {
                details = details.child(
                    div()
                        .id("sgf-comment")
                        .max_h(px(128. * self.zoom))
                        .overflow_y_scroll()
                        .child(clipped(comment, 16_384)),
                );
            }
            let parent = c.previous();
            let siblings = parent.map(|p| c.document.children(p).count()).unwrap_or(0);
            let (label, branch) = if c.document.children(c.selected).count() > 1 {
                ("Continue:", Some(c.selected))
            } else if siblings > 1 {
                ("Variation:", parent)
            } else {
                ("", None)
            };
            if let Some(branch) = branch {
                let count = c.document.children(branch).count();
                let selected_index = c
                    .document
                    .children(branch)
                    .position(|n| n == c.selected)
                    .unwrap_or(0);
                // Keep even adversarial records from creating unbounded UI elements.
                let start = selected_index.saturating_sub(6);
                let mut variations = div().flex().flex_wrap().gap_2().items_center().child(label);
                for (index, node) in c.document.children(branch).enumerate().skip(start).take(12) {
                    let name = c
                        .document
                        .text(node, "N")
                        .unwrap_or_else(|| format!("{}", index + 1));
                    variations = variations.child(
                        div()
                            .id(("sgf-variation", node))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .when(c.selected == node, |d| d.bg(theme.selection))
                            .hover(|d| d.bg(theme.selection))
                            .child(clipped(name, 48))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(Some(node), cx);
                                this.focus.focus(window);
                            })),
                    );
                }
                if count > 12 {
                    variations = variations.child(format!("{count} variations · ↑/↓ to cycle"));
                }
                details = details.child(variations);
            }
        }
        let board = if let Some(error) = &self.error {
            div()
                .p_4()
                .child(format!("{error}\nSource is available above."))
                .into_any_element()
        } else if let Some(image) = &self.image {
            let image = image.clone();
            let image_size = image.size(0).map(|p| px(p.0 as f32));
            // Paint into the space allocated by flex layout. An image element's
            // intrinsic size must not determine the board viewport's height.
            canvas(
                move |bounds, _, _| viewport::fit(bounds, image_size),
                move |bounds, fitted, window, _| {
                    if let Some(fitted) = fitted {
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            let _ = window.paint_image(fitted, Default::default(), image, 0, false);
                        });
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element()
        } else {
            div()
                .p_4()
                .text_color(theme.text_muted)
                .child(if ready {
                    "Rendering board…"
                } else {
                    "Opening SGF…"
                })
                .into_any_element()
        };
        div()
            .key_context("SgfPreview")
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus.focus(window)),
            )
            .child(controls)
            .child(
                div()
                    .id("sgf-board")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(board),
            )
            .child(details)
            .into_any_element()
    }
}
#[derive(Clone, Copy)]
enum Motion {
    First,
    Previous,
    Next,
    Last,
    Sibling(i32),
    Game(i32),
}
fn clipped(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
impl Focusable for SgfView {
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
impl Render for SgfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        self.request(dark, cx);
        let body = if self.show_raw {
            self.raw.as_ref().unwrap().clone().into_any_element()
        } else {
            self.preview(theme, cx)
        };
        div()
            .key_context("Sgf")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .on_action(cx.listener(|this, _: &ToggleSource, window, cx| this.toggle(window, cx)))
            .on_action(
                cx.listener(|this, _: &Next, window, cx| this.navigate(Motion::Next, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Previous, window, cx| {
                this.navigate(Motion::Previous, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &First, window, cx| this.navigate(Motion::First, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Last, window, cx| this.navigate(Motion::Last, window, cx)),
            )
            .on_action(cx.listener(|this, _: &NextVariation, window, cx| {
                this.navigate(Motion::Sibling(1), window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousVariation, window, cx| {
                this.navigate(Motion::Sibling(-1), window, cx)
            }))
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .text_xs()
                    .child("Go record")
                    .child(
                        div()
                            .id("sgf-source")
                            .cursor_pointer()
                            .child(if self.show_raw {
                                "Preview ⇧⌘V"
                            } else {
                                "Source ⇧⌘V"
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
