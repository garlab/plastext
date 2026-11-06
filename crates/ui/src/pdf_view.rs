mod cache;
mod nearby;
mod presentation;
mod viewport;

use cache::Tiles;
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, Entity, FocusHandle, Focusable, KeyBinding,
    MouseButton, Pixels, Point, RenderImage, SharedString, Subscription, Task, Window, actions,
    canvas, div, fill, point, prelude::*, px, rgb,
};
use nearby::{Nearby, PrefetchViewport};
use plastext_buffer::{FileSource, OpenOptions};
use plastext_editor::{HexView, TextStyle, Theme};
use plastext_platform::pdf::{Cancel, Document, PageInfo, Tile};
use presentation::Presentation;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use viewport::Viewport;

use crate::actions::TogglePreview as ToggleBinary;
actions!(
    pdf_view,
    [NextPage, PreviousPage, FirstPage, LastPage, Reload]
);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-shift-v", ToggleBinary, Some("Pdf && !Tex")),
        KeyBinding::new("right", NextPage, Some("PdfPreview")),
        KeyBinding::new("left", PreviousPage, Some("PdfPreview")),
        KeyBinding::new("home", FirstPage, Some("PdfPreview")),
        KeyBinding::new("end", LastPage, Some("PdfPreview")),
        KeyBinding::new("cmd-r", Reload, Some("PdfPreview && !Tex")),
    ]);
}

struct Pending {
    cancel: Cancel,
    tile: Option<Tile>,
    prefetch: bool,
    _task: Task<()>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct PdfView {
    path: PathBuf,
    source: Arc<FileSource>,
    style: TextStyle,
    focus: FocusHandle,
    raw: Option<Entity<HexView>>,
    raw_observer: Option<Subscription>,
    binary: bool,
    visible: bool,
    document: Option<Arc<Document>>,
    /// Keep the snapshot's disk reservation while Binary retains its descriptor,
    /// including during a reload that may fail.
    source_snapshot: Option<Arc<Document>>,
    info: Option<PageInfo>,
    page: u32,
    pages: Option<u32>,
    zoom: f32,
    fit_page: bool,
    pan: Point<f64>,
    pending: Option<Pending>,
    presentation: Presentation,
    resize_task: Option<Task<()>>,
    nearby: Nearby,
    prefetch_viewport: Option<(PrefetchViewport, usize)>,
    prefetch_blocked: bool,
    prefetch_retry: Option<Task<()>>,
    generation: u64,
    error: Option<SharedString>,
    embedded: bool,
    ready: bool,
    prefetch_suspended: bool,
    budget_sponsored: bool,
}

impl PdfView {
    pub fn new(
        path: PathBuf,
        source: Arc<FileSource>,
        style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        Tiles::init(cx);
        let owner = cx.entity_id();
        cx.on_release(move |this, cx| {
            this.pending = None;
            Tiles::set_active(owner, false, cx);
        })
        .detach();
        Self {
            path,
            source,
            style,
            focus: cx.focus_handle(),
            raw: None,
            raw_observer: None,
            binary: false,
            visible: false,
            document: None,
            source_snapshot: None,
            info: None,
            page: 1,
            pages: None,
            zoom: 1.,
            fit_page: false,
            pan: point(0., 0.),
            pending: None,
            presentation: Presentation::default(),
            resize_task: None,
            nearby: Nearby::default(),
            prefetch_viewport: None,
            prefetch_blocked: false,
            prefetch_retry: None,
            generation: 0,
            error: None,
            embedded: false,
            ready: false,
            prefetch_suspended: false,
            budget_sponsored: false,
        }
    }

    pub(super) fn compiled(
        path: PathBuf,
        source: Arc<FileSource>,
        output: &plastext_platform::tex::Compiled,
        style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(path, source, style, cx);
        view.embedded = true;
        view.document = Some(output.document.clone());
        view.source_snapshot = view.document.clone();
        view.info = Some(output.info);
        view.pages = Some(output.info.pages);
        view.nearby.visit(1, output.info.pages);
        view.nearby.insert(output.info);
        view
    }
    pub(super) fn ready(&self) -> bool {
        self.ready && self.error.is_none()
    }
    /// A replacement borrows the old view's reserved half-cache until the swap.
    /// Counting it as another pane would lower both raster scales mid-rebuild.
    pub(super) fn stage(&mut self, sponsored: bool) {
        self.budget_sponsored = sponsored;
        self.prefetch_suspended = true;
    }
    pub(super) fn suspend_prefetch(&mut self, suspend: bool, cx: &mut Context<Self>) {
        self.prefetch_suspended = suspend;
        if suspend {
            self.cancel_prefetch();
            Tiles::retain_prefetched(cx.entity_id(), &[], cx);
        }
        cx.notify();
    }
    pub(super) fn publish(&mut self, cx: &mut Context<Self>) {
        self.budget_sponsored = false;
        self.prefetch_suspended = false;
        Tiles::set_active(cx.entity_id(), self.visible, cx);
        cx.notify();
    }
    pub(super) fn reading_state(&self) -> (u32, bool, Point<f64>) {
        (self.page, self.fit_page, self.pan)
    }
    pub(super) fn restore_reading_state(
        &mut self,
        state: (u32, bool, Point<f64>),
        cx: &mut Context<Self>,
    ) {
        let state = (
            state.0.min(self.pages.unwrap_or(u32::MAX)),
            state.1,
            state.2,
        );
        if self.reading_state() == state {
            return;
        }
        self.go_to(state.0, cx);
        self.fit_page = state.1;
        self.pan = state.2;
        self.ready = false;
        cx.notify();
    }

    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        cx.new(|cx| {
            let mut view = Self::new(
                self.path.clone(),
                self.source.clone(),
                self.style.clone(),
                cx,
            );
            view.document = self.document.clone();
            view.source_snapshot = self.source_snapshot.clone();
            view.info = self.info;
            view.nearby = self.nearby.clone();
            view.page = self.page;
            view.pages = self.pages;
            view.zoom = self.zoom;
            view.fit_page = self.fit_page;
            view.pan = self.pan;
            view.binary = self.binary;
            view.embedded = self.embedded;
            if let Some(raw) = &self.raw {
                let raw = raw.update(cx, |raw, cx| raw.duplicate(cx));
                view.raw_observer = Some(cx.observe(&raw, |_, _, cx| cx.notify()));
                view.raw = Some(raw);
            }
            view
        })
    }

    pub fn set_zoom(&mut self, factor: f32, font_size: Pixels, cx: &mut Context<Self>) {
        if self.zoom != factor {
            self.ready = false;
        }
        self.zoom = factor;
        if let Some(raw) = &self.raw {
            raw.update(cx, |raw, cx| raw.set_font_size(font_size, cx));
        }
        cx.notify();
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.cancel();
            self.reset_presentation(cx);
        }
        Tiles::set_active(
            cx.entity_id(),
            visible && !self.binary && !self.budget_sponsored,
            cx,
        );
    }
    pub fn notice(&self) -> Option<SharedString> {
        if self.binary {
            None
        } else {
            self.error.clone()
        }
    }
    pub fn summary(&self, cx: &App) -> String {
        if self.binary {
            return self
                .raw
                .as_ref()
                .map(|raw| crate::tab_view::TabStatus::Hex(raw.read(cx).status()).summary())
                .unwrap_or_else(|| "PDF · Binary".into());
        }
        match self.info {
            Some(info) => format!("PDF · Page {} of {}", self.page, info.pages),
            None => "PDF".into(),
        }
    }
    // PDF page numbers are navigation positions; binary mode retains its own byte offset.
    pub fn position(&self) -> u64 {
        self.page as u64
    }
    pub fn scroll_to_offset(&mut self, page: u64, cx: &mut Context<Self>) {
        self.go_to(page.clamp(1, u32::MAX as u64) as u32, cx);
    }
    fn cancel(&mut self) {
        self.generation += 1;
        self.pending = None;
        self.prefetch_retry = None;
    }
    fn cancel_prefetch(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.prefetch)
        {
            self.cancel();
        }
    }
    fn reset_presentation(&mut self, cx: &mut Context<Self>) {
        self.ready = false;
        self.presentation = Presentation::default();
        self.resize_task = None;
        self.prefetch_blocked = false;
        Tiles::pin(cx.entity_id(), &[], cx);
    }

    fn wake_after_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize_task.is_some() {
            return;
        }
        let Some(mut delay) = self.presentation.remaining(Instant::now()) else {
            return;
        };
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(delay).await;
                let remaining = this.update(cx, |this, cx| {
                    let remaining = this.presentation.remaining(Instant::now());
                    if remaining.is_none() {
                        this.resize_task = None;
                        cx.notify();
                    }
                    remaining
                });
                match remaining {
                    Ok(Some(next)) => delay = next,
                    _ => break,
                }
            }
        }));
    }
    fn go_to(&mut self, page: u32, cx: &mut Context<Self>) {
        let page = page.clamp(1, self.pages.unwrap_or(u32::MAX));
        if page == self.page {
            return;
        }
        self.cancel();
        self.reset_presentation(cx);
        self.page = page;
        self.nearby.visit(page, self.pages.unwrap_or(u32::MAX));
        self.info = self.nearby.info(page);
        self.prefetch_blocked = false;
        let mut keep = self.nearby.candidates();
        keep.push(page);
        Tiles::retain_prefetched(cx.entity_id(), &keep, cx);
        self.pan = point(0., 0.);
        self.error = None;
        cx.notify();
    }
    fn reload(&mut self, _: &Reload, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel();
        self.reset_presentation(cx);
        self.document = None;
        self.info = None;
        self.nearby = Nearby::default();
        self.prefetch_viewport = None;
        self.prefetch_blocked = false;
        self.error = None;
        Tiles::remove(cx.entity_id(), cx);
        cx.notify();
    }
    fn toggle_binary(&mut self, _: &ToggleBinary, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel();
        self.reset_presentation(cx);
        self.binary = !self.binary;
        Tiles::set_active(cx.entity_id(), self.visible && !self.binary, cx);
        if self.binary {
            let raw = self.raw.get_or_insert_with(|| {
                cx.new(|cx| HexView::new(self.source.clone(), self.style.clone(), cx))
            });
            raw.update(cx, |raw, cx| {
                raw.set_font_size(self.style.font_size * self.zoom, cx)
            });
            if self.raw_observer.is_none() {
                self.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
            raw.focus_handle(cx).focus(window);
            Tiles::remove(cx.entity_id(), cx);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }

    fn load_page(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.error.is_some() || self.binary {
            return;
        }
        if !plastext_platform::pdf::supported() {
            self.error = Some("PDF preview is currently available on macOS; use Binary".into());
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let document = self.document.clone();
        let source = self.source.clone();
        let path = self.path.clone();
        let page = self.page;
        let generation = self.generation;
        let work = cx.background_executor().spawn(async move {
            if let Some(document) = document {
                let info = document.page(page, &flag);
                Ok((document, info, source))
            } else {
                let document = Arc::new(Document::snapshot(&path, &flag)?);
                let info = document.page(1, &flag).and_then(|first| {
                    let page = page.min(first.pages);
                    if page == 1 {
                        Ok(first)
                    } else {
                        document.page(page, &flag)
                    }
                });
                let source = Arc::new(
                    FileSource::from_file(
                        document.file().map_err(|e| e.to_string())?,
                        OpenOptions {
                            large_file_threshold: 0,
                            block_cache: 8 * 1024 * 1024,
                        },
                    )
                    .map_err(|e| e.to_string())?,
                );
                Result::<_, String>::Ok((document, info, source))
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok((document, info, source)) => {
                        if !Arc::ptr_eq(&this.source, &source) {
                            this.source = source;
                            if let Some(old) = this.raw.take() {
                                let offset = old.read(cx).position();
                                let raw = cx.new(|cx| {
                                    let mut raw =
                                        HexView::new(this.source.clone(), this.style.clone(), cx);
                                    raw.set_font_size(this.style.font_size * this.zoom, cx);
                                    raw.scroll_to_offset(offset, cx);
                                    raw
                                });
                                this.raw_observer = Some(cx.observe(&raw, |_, _, cx| cx.notify()));
                                this.raw = Some(raw);
                            }
                        }
                        this.source_snapshot = Some(document.clone());
                        this.document = Some(document);
                        match info {
                            Ok(info) => {
                                this.page = info.page;
                                this.pages = Some(info.pages);
                                this.info = Some(info);
                                this.nearby.visit(info.page, info.pages);
                                this.nearby.insert(info);
                            }
                            Err(error) => this.error = Some(error.into()),
                        }
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            });
        });
        self.pending = Some(Pending {
            cancel,
            tile: None,
            prefetch: false,
            _task: task,
        });
    }

    fn request_tile(
        &mut self,
        tile: Tile,
        prefetch: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending.is_some() || self.error.is_some() {
            return;
        }
        let Some(document) = self.document.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let generation = self.generation;
        let handle = window.window_handle();
        let work = cx.background_executor().spawn(async move {
            if prefetch {
                document.prefetch_tile(tile, &flag)
            } else {
                document.tile(tile, &flag)
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok(pixels) => {
                        let buffer =
                            image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.bgra)
                                .unwrap();
                        let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                        if prefetch {
                            this.prefetch_blocked = !Tiles::insert_prefetched(
                                cx.entity_id(),
                                pixels.tile,
                                image,
                                handle,
                                cx,
                            );
                        } else {
                            Tiles::insert(cx.entity_id(), pixels.tile, image, handle, cx);
                            this.presentation.received(pixels.tile);
                        }
                    }
                    Err(error) if prefetch => this.prefetch_failed(tile.page, &error, cx),
                    Err(error) if this.presentation.is_current(tile) => {
                        this.error = Some(error.into())
                    }
                    Err(_) => {} // An obsolete resolution must not fail the new preview.
                }
                cx.notify();
            });
        });
        self.pending = Some(Pending {
            cancel,
            tile: Some(tile),
            prefetch,
            _task: task,
        });
    }

    fn prefetch_failed(&mut self, page: u32, error: &str, cx: &mut Context<Self>) {
        if error == plastext_platform::pdf::PREFETCH_INTERRUPTED {
            // A visible request in another pane preempted this one. Retry after
            // a pause, without a busy render/reject loop or surfacing an error.
            self.prefetch_retry = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.prefetch_retry = None;
                    cx.notify();
                });
            }));
        } else {
            self.nearby.failed(page);
        }
    }

    fn request_prefetch_info(&mut self, page: u32, cx: &mut Context<Self>) {
        let Some(document) = self.document.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let generation = self.generation;
        let work = cx
            .background_executor()
            .spawn(async move { document.prefetch_page(page, &flag) });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok(info) => this.nearby.insert(info),
                    Err(error) => this.prefetch_failed(page, &error, cx),
                }
                cx.notify();
            });
        });
        self.pending = Some(Pending {
            cancel,
            tile: None,
            prefetch: true,
            _task: task,
        });
    }

    fn prefetch(&mut self, viewport: PrefetchViewport, window: &Window, cx: &mut Context<Self>) {
        if !self.visible
            || self.binary
            || self.prefetch_suspended
            || self.pending.is_some()
            || self.error.is_some()
            || self.prefetch_retry.is_some()
            || self.prefetch_blocked
        {
            return;
        }
        let candidates = self.nearby.candidates();
        // Read the small metadata records first, so fast repeated page turns
        // need not wait for an earlier neighbor's raster work to finish.
        for &page in &candidates {
            if self.nearby.info(page).is_none() {
                self.request_prefetch_info(page, cx);
                return;
            }
        }
        let pages: Vec<_> = candidates
            .into_iter()
            .filter_map(|page| self.nearby.info(page))
            .collect();
        let plan = viewport.plan(
            &pages,
            Tiles::tile_budget(cx),
            Tiles::prefetch_space(cx.entity_id(), cx),
        );
        let wanted: Vec<_> = plan.iter().flatten().copied().collect();
        Tiles::prepare_prefetch(cx.entity_id(), &wanted, cx);
        for tiles in plan {
            if let Some(&tile) = tiles
                .iter()
                .find(|&&tile| !Tiles::contains(cx.entity_id(), tile, cx))
            {
                self.request_tile(tile, true, window, cx);
                return;
            }
            self.nearby.ready(tiles[0].page, tiles);
        }
    }

    fn layout(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageLayout {
        if !self.visible {
            return PageLayout::default();
        }
        let Some(info) = self.info else {
            self.load_page(cx);
            return PageLayout::default();
        };
        let prefetch_viewport = PrefetchViewport {
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
            dpr: window.scale_factor() as f64,
            zoom: self.zoom as f64,
            fit_page: self.fit_page,
        };
        let prefetch_key = (prefetch_viewport, Tiles::tile_budget(cx));
        if self.prefetch_viewport != Some(prefetch_key) {
            self.cancel_prefetch();
            Tiles::retain_prefetched(cx.entity_id(), &[], cx);
            self.nearby.clear_previews();
            self.prefetch_blocked = false;
            self.prefetch_viewport = Some(prefetch_key);
        }
        let mut viewport = Viewport::new(
            info,
            bounds.size.width.into(),
            bounds.size.height.into(),
            window.scale_factor() as f64,
            self.zoom as f64,
            self.fit_page,
            self.pan,
        );
        if !viewport.fit_budget(Tiles::tile_budget(cx)) {
            self.cancel();
            self.error = Some(
                "Too many PDF previews for the tile budget; close a pane or use Binary".into(),
            );
            return PageLayout::default();
        }
        self.pan = viewport.pan;
        let now = Instant::now();
        self.presentation.target(info.page, viewport.scale(), now);
        let preview = self.nearby.preview(info.page);
        if preview
            .iter()
            .all(|&tile| Tiles::contains(cx.entity_id(), tile, cx))
        {
            self.presentation.seed(preview);
        }
        let settling = self.presentation.remaining(now).is_some();
        let visible = viewport.tiles(info.page);
        if !settling
            && self
                .pending
                .as_ref()
                .filter(|pending| !pending.prefetch)
                .and_then(|p| p.tile)
                .is_some_and(|tile| !visible.contains(&tile))
        {
            self.cancel();
        }
        let mut missing = None;
        let mut available = Vec::new();
        for &tile in &visible {
            if Tiles::get(cx.entity_id(), tile, cx).is_some() {
                available.push(tile);
            } else if missing.is_none() {
                missing = Some(tile);
            }
        }
        let displayed = self.presentation.select(&visible, &available);
        Tiles::pin(cx.entity_id(), displayed, cx);
        let mut tiles = Vec::new();
        for &tile in displayed {
            if let Some(image) = Tiles::get(cx.entity_id(), tile, cx) {
                let image_size = image.size(0);
                let rect = viewport.tile_bounds(
                    tile,
                    image_size.width.0 as f64,
                    image_size.height.0 as f64,
                    bounds.origin,
                );
                tiles.push((rect, image));
            }
        }
        let ready = missing.is_none() && !settling;
        if self.ready != ready {
            self.ready = ready;
            cx.notify();
        }
        if let Some(tile) = missing {
            self.cancel_prefetch();
            if !settling {
                self.request_tile(tile, false, window, cx);
            }
        } else if !settling {
            if self.pan == point(0., 0.) {
                self.nearby.ready(info.page, visible);
            }
            self.prefetch(prefetch_viewport, window, cx);
        }
        if settling {
            self.wake_after_resize(cx);
        }
        PageLayout {
            paper: Some(viewport.paper_bounds(bounds.origin)),
            tiles,
        }
    }
}

impl Focusable for PdfView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.binary
            && let Some(raw) = &self.raw
        {
            return raw.focus_handle(cx);
        }
        self.focus.clone()
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let body: AnyElement = if self.binary {
            self.raw.as_ref().unwrap().clone().into_any_element()
        } else {
            let view = cx.entity();
            div()
                .key_context("PdfPreview")
                .track_focus(&self.focus)
                .size_full()
                .relative()
                .overflow_hidden()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, _| this.focus.focus(window)),
                )
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let delta = event.delta.pixel_delta(px(32.));
                    this.pan.x -= f64::from(delta.x);
                    this.pan.y -= f64::from(delta.y);
                    cx.notify();
                }))
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            view.update(cx, |this, cx| this.layout(bounds, window, cx))
                        },
                        move |bounds, layout: PageLayout, window, _| {
                            let bounds = layout
                                .paper
                                .map_or(bounds, |paper| paper.intersect(&bounds));
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                if let Some(paper) = layout.paper {
                                    window.paint_quad(fill(paper, rgb(0xffffff)));
                                }
                                for (bounds, image) in layout.tiles {
                                    let _ = window.paint_image(
                                        bounds,
                                        Default::default(),
                                        image,
                                        0,
                                        false,
                                    );
                                }
                            });
                        },
                    )
                    .size_full(),
                )
                .when(self.info.is_none() || self.error.is_some(), |element| {
                    element.child(
                        div()
                            .absolute()
                            .top_4()
                            .left_4()
                            .right_4()
                            .p_3()
                            .bg(theme.surface)
                            .text_color(theme.text)
                            .child(self.error.clone().unwrap_or_else(|| {
                                if self.document.is_some() {
                                    format!("Loading page {}…", self.page).into()
                                } else {
                                    "Opening PDF…".into()
                                }
                            })),
                    )
                })
                .into_any_element()
        };
        div()
            .key_context("Pdf")
            .size_full()
            .flex()
            .flex_col()
            .when(!self.embedded, |d| {
                d.on_action(cx.listener(Self::toggle_binary))
                    .on_action(cx.listener(Self::reload))
            })
            .on_action(
                cx.listener(|this, _: &NextPage, _, cx| {
                    this.go_to(this.page.saturating_add(1), cx)
                }),
            )
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| {
                this.go_to(this.page.saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &FirstPage, _, cx| this.go_to(1, cx)))
            .on_action(cx.listener(|this, _: &LastPage, _, cx| {
                if let Some(info) = this.info {
                    this.go_to(info.pages, cx);
                }
            }))
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .border_b_1()
                    .border_color(theme.border)
                    .child("PDF")
                    .when(!self.binary, |bar| {
                        bar.child(
                            div()
                                .id("pdf-previous")
                                .cursor_pointer()
                                .child("‹")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.go_to(this.page.saturating_sub(1), cx);
                                    this.focus.focus(window);
                                })),
                        )
                        .child(self.info.map_or_else(
                            || "…".to_string(),
                            |info| format!("{} / {}", self.page, info.pages),
                        ))
                        .child(div().id("pdf-next").cursor_pointer().child("›").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.go_to(this.page.saturating_add(1), cx);
                                this.focus.focus(window);
                            }),
                        ))
                        .child(
                            div()
                                .id("pdf-fit")
                                .cursor_pointer()
                                .child(if self.fit_page {
                                    "Fit width"
                                } else {
                                    "Fit page"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.fit_page = !this.fit_page;
                                    this.pan = point(0., 0.);
                                    this.focus.focus(window);
                                    cx.notify();
                                })),
                        )
                        .when(!self.embedded, |bar| {
                            bar.child(
                                div()
                                    .id("pdf-reload")
                                    .cursor_pointer()
                                    .child("Reload")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reload(&Reload, window, cx);
                                        this.focus.focus(window);
                                    })),
                            )
                        })
                    })
                    .child(div().flex_1())
                    .when(!self.embedded, |bar| {
                        bar.child(
                            div()
                                .id("pdf-toggle")
                                .cursor_pointer()
                                .child(if self.binary {
                                    "Preview  ⇧⌘V"
                                } else {
                                    "Binary  ⇧⌘V"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_binary(&ToggleBinary, window, cx)
                                })),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[derive(Default)]
struct PageLayout {
    paper: Option<Bounds<Pixels>>,
    tiles: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
}
