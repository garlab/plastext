mod cache;
mod playback;
mod render;
mod viewport;

use cache::Cache;
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Pixels, Point, RenderImage,
    SharedString, Subscription, Task, Window, actions, point, prelude::*,
};
use plastext_assets::Preview;
use plastext_buffer::{FileSource, LineIndex};
use plastext_editor::{HexView, TextStyle, TextView};
use plastext_platform::images::{self, Document, Info, Region};
use playback::Playback;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use viewport::Viewport;

actions!(image_view, [ToggleSource, TogglePlayback]);
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-shift-v", ToggleSource, Some("Image")),
        KeyBinding::new("space", TogglePlayback, Some("ImagePreview")),
    ]);
}

pub(crate) fn render_image(mut preview: Preview) -> (Arc<RenderImage>, u32, u32) {
    for pixel in preview.rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(preview.width, preview.height, preview.rgba).unwrap();
    (
        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
        preview.original_width,
        preview.original_height,
    )
}
struct Pending {
    plan: Option<Vec<Region>>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
struct Ready {
    regions: Vec<Region>,
    delay: u32,
}
#[derive(Default)]
struct Layout {
    paper: Option<Bounds<Pixels>>,
    images: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
    pixel: f32,
}

pub struct ImageView {
    path: PathBuf,
    source: Arc<FileSource>,
    lines: Option<Arc<LineIndex>>,
    style: TextStyle,
    raw_text: Option<Entity<TextView>>,
    raw_hex: Option<Entity<HexView>>,
    raw_observer: Option<Subscription>,
    show_raw: bool,
    focus: FocusHandle,
    visible: bool,
    document: Option<Arc<Document>>,
    info: Option<Info>,
    error: Option<SharedString>,
    failed: Option<Vec<Region>>,
    pending: Option<Pending>,
    wake: Option<Task<()>>,
    ready: Option<Ready>,
    displayed: Vec<Region>,
    playback: Playback,
    zoom: Option<f64>,
    document_zoom: f32,
    pan: Point<f64>,
    viewport: Option<Viewport>,
    bounds: Bounds<Pixels>,
    drag: Option<Point<Pixels>>,
    geometry: Vec<Region>,
    settle_until: Instant,
    #[cfg(target_os = "macos")]
    pinch: Option<(plastext_platform::gestures::PinchMonitor, Task<()>)>,
}
impl ImageView {
    pub fn new(
        path: PathBuf,
        source: Arc<FileSource>,
        lines: Option<Arc<LineIndex>>,
        style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        Cache::active(cx.entity_id(), true, cx);
        let owner = cx.entity_id();
        cx.on_release(move |_, cx| Cache::active(owner, false, cx))
            .detach();
        Self {
            path,
            source,
            lines,
            style,
            raw_text: None,
            raw_hex: None,
            raw_observer: None,
            show_raw: false,
            focus: cx.focus_handle(),
            visible: true,
            document: None,
            info: None,
            error: None,
            failed: None,
            pending: None,
            wake: None,
            ready: None,
            displayed: Vec::new(),
            playback: Playback::default(),
            zoom: None,
            document_zoom: 1.,
            pan: point(0., 0.),
            viewport: None,
            bounds: Bounds::default(),
            drag: None,
            geometry: Vec::new(),
            settle_until: Instant::now(),
            #[cfg(target_os = "macos")]
            pinch: None,
        }
    }
    pub fn summary(&self) -> String {
        let mode = if self.show_raw {
            if self.lines.is_some() {
                "Source"
            } else {
                "Binary"
            }
        } else {
            "Image"
        };
        match &self.info {
            Some(info) => format!(
                "{} × {}    {}    {}    {}    {mode}",
                info.width,
                info.height,
                crate::tab_view::format_size(self.document.as_ref().map_or(0, |d| d.bytes)),
                info.format,
                info.color
            ),
            None => mode.into(),
        }
    }
    pub fn notice(&self) -> Option<SharedString> {
        self.error.clone()
    }
    pub fn set_zoom(&mut self, factor: f32, font_size: Pixels, cx: &mut Context<Self>) {
        self.style.font_size = font_size;
        if let Some(raw) = &self.raw_text {
            raw.update(cx, |v, cx| v.set_font_size(font_size, cx));
        }
        if let Some(raw) = &self.raw_hex {
            raw.update(cx, |v, cx| v.set_font_size(font_size, cx));
        }
        if self.document_zoom != factor {
            self.zoom_by((factor / self.document_zoom) as f64, None, cx);
            self.document_zoom = factor;
        }
    }
    fn zoom_by(&mut self, factor: f64, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if let Some(view) = &self.viewport {
            let zoom = (view.zoom * factor).clamp(1. / 100_000., 32.);
            let anchor = anchor
                .map(|p| {
                    point(
                        f64::from(p.x - self.bounds.origin.x),
                        f64::from(p.y - self.bounds.origin.y),
                    )
                })
                .unwrap_or_else(|| point(view.width / 2., view.height / 2.));
            self.pan = view.zoom_pan(zoom, anchor);
            self.zoom = Some(zoom);
            cx.notify();
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.release(cx);
        } else {
            Cache::active(cx.entity_id(), !self.show_raw, cx);
            self.playback.due = Instant::now() + self.playback.delay;
            cx.notify();
        }
    }
    fn release(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.pinch = None;
        }
        self.pending = None;
        self.wake = None;
        self.ready = None;
        self.displayed.clear();
        self.failed = None;
        self.drag = None;
        Cache::active(cx.entity_id(), false, cx);
    }
    #[cfg(target_os = "macos")]
    fn listen_for_pinch(&mut self, window: &Window, cx: &mut Context<Self>) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if self.pinch.is_some() || !self.visible || self.show_raw {
            return;
        }
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: GPUI provides a live NSView; entity lifetimes stay on the UI thread.
        let Some((monitor, events)) =
            (unsafe { plastext_platform::gestures::pinch_monitor(handle.ns_view) })
        else {
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        let position = point(gpui::px(event.x as f32), gpui::px(event.y as f32));
                        if this.visible && !this.show_raw && this.bounds.contains(&position) {
                            this.zoom_by(event.factor, Some(position), cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.pinch = Some((monitor, task));
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.error.is_some() {
            return;
        }
        let path = self.path.clone();
        let budget = Cache::frame_budget(cx);
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let work = cx.background_executor().spawn(async move {
            let (document, info) = Document::open(&path, &token)?;
            let region = Viewport::overview(&info, 0, budget);
            let frame = document.render(&info, vec![region], &token)?;
            Ok::<_, String>((document, info, convert(frame)))
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                match result {
                    Ok((document, info, (pixels, delay))) => {
                        this.document = Some(document);
                        this.info = Some(info);
                        this.playback.present(0, delay);
                        this.displayed = pixels.iter().map(|(r, _)| *r).collect();
                        for (r, image) in pixels {
                            Cache::insert(cx.entity_id(), r, image, cx);
                        }
                    }
                    Err(error) => {
                        this.error = Some(error.into());
                    }
                }
                cx.notify();
            });
        });
        self.pending = Some(Pending {
            plan: None,
            cancel,
            _task: task,
        });
    }
    fn request(&mut self, plan: Vec<Region>, next: bool, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.failed.as_ref() == Some(&plan) {
            return;
        }
        let Some(document) = self.document.clone() else {
            return;
        };
        let info = self.info.clone().unwrap();
        let missing: Vec<_> = plan
            .iter()
            .copied()
            .filter(|r| !Cache::contains(cx.entity_id(), *r, cx))
            .collect();
        if missing.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        let work = cx
            .background_executor()
            .spawn(async move { document.render(&info, missing, &token).map(convert) });
        let requested = plan.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                match result {
                    Ok((pixels, delay)) => {
                        this.error = None;
                        this.failed = None;
                        for (r, image) in pixels {
                            Cache::insert(cx.entity_id(), r, image, cx);
                        }
                        if next {
                            this.ready = Some(Ready {
                                regions: plan,
                                delay,
                            });
                        } else {
                            if this
                                .displayed
                                .first()
                                .is_none_or(|r| r.frame != plan[0].frame)
                            {
                                this.playback.present(plan[0].frame, delay);
                            }
                            this.displayed = plan;
                        }
                    }
                    Err(error) => {
                        this.failed = Some(plan);
                        this.error = Some(error.into());
                    }
                }
                cx.notify();
            });
        });
        self.pending = Some(Pending {
            plan: Some(requested),
            cancel,
            _task: task,
        });
    }
    fn wake_at(&mut self, when: Instant, cx: &mut Context<Self>) {
        if self.wake.is_some() {
            return;
        }
        self.wake = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(
                    when.saturating_duration_since(Instant::now())
                        .max(Duration::from_millis(1)),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.wake = None;
                cx.notify();
            });
        }));
    }
    fn layout(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Layout {
        self.bounds = bounds;
        if !self.visible || self.show_raw {
            return Layout::default();
        }
        let Some(info) = self.info.clone() else {
            self.load(cx);
            return Layout::default();
        };
        let mut viewport = Viewport::new(
            &info,
            bounds.size.width.into(),
            bounds.size.height.into(),
            window.scale_factor() as f64,
            self.zoom,
            self.pan,
        );
        let mut plan = viewport.plan(self.playback.frame, Cache::frame_budget(cx));
        self.pan = viewport.pan;
        let geometry: Vec<_> = plan.iter().map(|r| Region { frame: 0, ..*r }).collect();
        if self.geometry != geometry {
            self.geometry = geometry;
            self.settle_until = Instant::now() + Duration::from_millis(60);
            self.wake = None;
        }
        let settling = !self.displayed.is_empty() && Instant::now() < self.settle_until;
        let next = self.playback.next(&info);
        let next_plan = next.map(|frame| {
            plan.iter()
                .map(|r| Region { frame, ..*r })
                .collect::<Vec<_>>()
        });
        if let Some(pending) = &self.pending
            && let Some(request) = &pending.plan
            && *request != plan
            && Some(request) != next_plan.as_ref()
        {
            self.pending = None;
        }
        if self
            .ready
            .as_ref()
            .is_some_and(|ready| Some(&ready.regions) != next_plan.as_ref())
        {
            self.ready = None;
            self.wake = None;
        }
        if self.ready.is_some() && Instant::now() >= self.playback.due {
            let ready = self.ready.take().unwrap();
            if ready
                .regions
                .iter()
                .all(|r| Cache::contains(cx.entity_id(), *r, cx))
            {
                self.playback.present(ready.regions[0].frame, ready.delay);
                self.displayed = ready.regions.clone();
                plan = ready.regions;
                self.wake = None;
                cx.notify();
            }
        }
        let complete = plan.iter().all(|r| Cache::contains(cx.entity_id(), *r, cx));
        if complete {
            self.displayed = plan.clone();
        }
        let mut keep = self.displayed.clone();
        keep.extend(plan.iter().copied());
        if let Some(ready) = &self.ready {
            keep.extend(&ready.regions);
        }
        Cache::retain(cx.entity_id(), &keep, cx);
        let mut images = Vec::new();
        for &region in &self.displayed {
            if let Some(image) = Cache::get(cx.entity_id(), region, cx) {
                let rect = if region.x == 0
                    && region.y == 0
                    && (region.width, region.height) == info.scaled_size(region.scale)
                {
                    viewport.paper(bounds.origin)
                } else {
                    viewport.rect(region, bounds.origin)
                };
                images.push((rect, image));
            }
        }
        if settling {
            self.wake_at(self.settle_until, cx);
        } else if !complete {
            self.request(plan, false, cx);
        } else if self.ready.is_some() {
            self.wake_at(self.playback.due, cx);
        } else if let Some(next) = self.playback.next(&info) {
            self.request(
                plan.iter().map(|r| Region { frame: next, ..*r }).collect(),
                true,
                cx,
            );
        }
        let layout = Layout {
            paper: Some(viewport.paper(bounds.origin)),
            images,
            pixel: if info.format == "SVG" {
                0.
            } else {
                viewport.zoom as f32
            },
        };
        self.viewport = Some(viewport);
        layout
    }
    fn toggle_source(&mut self, _: &ToggleSource, window: &mut Window, cx: &mut Context<Self>) {
        self.show_raw = !self.show_raw;
        if self.show_raw {
            self.release(cx);
            if let Some(lines) = &self.lines {
                let raw = self.raw_text.get_or_insert_with(|| {
                    cx.new(|cx| {
                        TextView::new(
                            self.source.clone(),
                            lines.clone(),
                            self.style.clone(),
                            plastext_syntax::detect(&self.path, b""),
                            cx,
                        )
                    })
                });
                if self.raw_observer.is_none() {
                    self.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
                }
                raw.focus_handle(cx).focus(window);
            } else {
                let raw = self.raw_hex.get_or_insert_with(|| {
                    cx.new(|cx| HexView::new(self.source.clone(), self.style.clone(), cx))
                });
                if self.raw_observer.is_none() {
                    self.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
                }
                raw.focus_handle(cx).focus(window);
            }
        } else {
            Cache::active(cx.entity_id(), self.visible, cx);
            self.focus.focus(window);
            self.error = None;
            self.playback.due = Instant::now() + self.playback.delay;
        }
        cx.notify();
    }
    fn toggle_playback(&mut self, _: &TogglePlayback, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(info) = &self.info
            && info.frames > 1
        {
            self.playback.toggle(info);
            self.pending = None;
            self.ready = None;
            self.wake = None;
            self.failed = None;
            cx.notify();
        }
        self.focus.focus(window);
    }
    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let raw_text = self
            .raw_text
            .as_ref()
            .map(|raw| raw.update(cx, |v, cx| v.duplicate(cx)));
        let raw_hex = self
            .raw_hex
            .as_ref()
            .map(|raw| raw.update(cx, |v, cx| v.duplicate(cx)));
        cx.new(|cx| {
            let mut v = Self::new(
                self.path.clone(),
                self.source.clone(),
                self.lines.clone(),
                self.style.clone(),
                cx,
            );
            v.document = self.document.clone();
            v.info = self.info.clone();
            v.zoom = self.zoom;
            v.document_zoom = self.document_zoom;
            v.pan = self.pan;
            v.playback.frame = self.playback.frame;
            v.playback.playing = self.playback.playing;
            v.show_raw = self.show_raw;
            v.raw_text = raw_text;
            v.raw_hex = raw_hex;
            if let Some(raw) = &v.raw_text {
                v.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
            if let Some(raw) = &v.raw_hex {
                v.raw_observer = Some(cx.observe(raw, |_, _, cx| cx.notify()));
            }
            Cache::active(cx.entity_id(), !v.show_raw, cx);
            v
        })
    }
}
type Converted = (Vec<(Region, Arc<RenderImage>)>, u32);
fn convert(frame: images::Frame) -> Converted {
    (
        frame
            .pixels
            .into_iter()
            .map(|p| {
                let r = p.region;
                (
                    r,
                    render_image(Preview {
                        rgba: p.rgba,
                        width: r.width,
                        height: r.height,
                        original_width: r.width,
                        original_height: r.height,
                    })
                    .0,
                )
            })
            .collect(),
        frame.delay_ms,
    )
}
impl Focusable for ImageView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.show_raw {
            if let Some(raw) = &self.raw_text {
                return raw.focus_handle(cx);
            }
            if let Some(raw) = &self.raw_hex {
                return raw.focus_handle(cx);
            }
        }
        self.focus.clone()
    }
}
