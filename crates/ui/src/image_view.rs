use gpui::{
    App, Context, FocusHandle, Focusable, Pixels, RenderImage, ScrollHandle, SharedString, Size,
    Window, canvas, div, img, prelude::*, px,
};
use plastext_assets::Preview;
use plastext_editor::Theme;
use std::{path::PathBuf, sync::Arc};

const PADDING: f32 = 24.;

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

pub struct ImageView {
    path: PathBuf,
    image: Result<(Arc<RenderImage>, u32, u32), String>,
    focus: FocusHandle,
    zoom: Option<f32>,
    document_zoom: f32,
    viewport: Size<Pixels>,
    pan: ScrollHandle,
}
impl ImageView {
    pub fn new(path: PathBuf, preview: Result<Preview, String>, cx: &mut Context<Self>) -> Self {
        Self {
            path,
            image: preview.map(render_image),
            focus: cx.focus_handle(),
            zoom: None,
            document_zoom: 1.,
            viewport: Size::default(),
            pan: ScrollHandle::new(),
        }
    }
    pub fn summary(&self) -> String {
        match &self.image {
            Ok((image, w, h)) => format!(
                "{w} × {h}    {}",
                if (image.size(0).width.0 as u32) < *w || (image.size(0).height.0 as u32) < *h {
                    "Downscaled preview"
                } else {
                    "Image"
                }
            ),
            Err(_) => "Image".into(),
        }
    }
    pub fn notice(&self) -> Option<SharedString> {
        self.image.as_ref().err().cloned().map(Into::into)
    }
    pub fn set_zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        if self.document_zoom == factor {
            return;
        }
        // Keyboard zoom starts from the fitted size already on screen. New
        // panes still start in Fit, whatever the window's document zoom is.
        if self.zoom.is_none() && self.viewport.height > px(0.) {
            self.zoom = Some(self.display_scale() / self.document_zoom);
        }
        self.document_zoom = factor;
        cx.notify();
    }

    fn display_scale(&self) -> f32 {
        if let Some(zoom) = self.zoom {
            return zoom * self.document_zoom;
        }
        let Ok((_, width, height)) = &self.image else {
            return 1.;
        };
        let available_width = (f32::from(self.viewport.width) - PADDING * 2.).max(0.);
        let available_height = (f32::from(self.viewport.height) - PADDING * 2.).max(0.);
        (available_width / (*width).max(1) as f32)
            .min(available_height / (*height).max(1) as f32)
            .min(1.)
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let scale = (self.display_scale() * factor).clamp(0.01, 16.);
        self.zoom = Some(scale / self.document_zoom);
        cx.notify();
    }

    pub fn duplicate(&self, cx: &mut Context<Self>) -> gpui::Entity<Self> {
        let image = self.image.clone();
        let path = self.path.clone();
        let zoom = self.zoom;
        let document_zoom = self.document_zoom;
        cx.new(|cx| Self {
            path,
            image,
            focus: cx.focus_handle(),
            zoom,
            document_zoom,
            viewport: Size::default(),
            pan: ScrollHandle::new(),
        })
    }
}
impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        let image = match &self.image {
            Ok((image, w, h)) if self.zoom.is_some() => {
                let zoom = self.zoom.unwrap() * self.document_zoom;
                div()
                    .id("image-pan")
                    .absolute()
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.pan)
                    .p(px(PADDING))
                    .child(
                        div()
                            .size_full()
                            .min_w(px(*w as f32 * zoom))
                            .min_h(px(*h as f32 * zoom))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                img(image.clone())
                                    .flex_none()
                                    .w(px(*w as f32 * zoom))
                                    .h(px(*h as f32 * zoom)),
                            ),
                    )
                    .into_any_element()
            }
            Ok((image, w, h)) => div()
                // Out-of-flow content cannot enlarge the viewport to the
                // image's intrinsic height. Cap both axes at natural size.
                .absolute()
                .size_full()
                .p(px(PADDING))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(image.clone())
                        .size_full()
                        .max_w(px(*w as f32))
                        .max_h(px(*h as f32))
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            Err(error) => div()
                .p_6()
                .text_color(theme.text_muted)
                .child(error.clone())
                .into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .gap_4()
                    .items_center()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .text_xs()
                    .child(self.summary())
                    .child(
                        div()
                            .id("image-fit")
                            .cursor_pointer()
                            .child("Fit")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = None;
                                this.pan.set_offset(Default::default());
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("image-one")
                            .cursor_pointer()
                            .child("1:1")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = Some(1. / this.document_zoom);
                                this.pan.set_offset(Default::default());
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("image-out")
                            .cursor_pointer()
                            .child("−")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_by(1. / 1.5, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("image-in")
                            .cursor_pointer()
                            .child("+")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_by(1.5, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(image)
                    .child(
                        canvas(
                            {
                                let view = cx.entity().downgrade();
                                move |bounds, _, cx| {
                                    let _ = view.update(cx, |this, _| this.viewport = bounds.size);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
    }
}
