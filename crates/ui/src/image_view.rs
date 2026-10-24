use gpui::{
    App, Context, FocusHandle, Focusable, RenderImage, SharedString, Window, div, img, prelude::*,
    px,
};
use plastext_assets::Preview;
use plastext_editor::Theme;
use std::{path::PathBuf, sync::Arc};

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
}
impl ImageView {
    pub fn new(path: PathBuf, preview: Result<Preview, String>, cx: &mut Context<Self>) -> Self {
        Self {
            path,
            image: preview.map(render_image),
            focus: cx.focus_handle(),
            zoom: None,
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
    pub fn duplicate(&self, cx: &mut Context<Self>) -> gpui::Entity<Self> {
        let image = self.image.clone();
        let path = self.path.clone();
        let zoom = self.zoom;
        cx.new(|cx| Self {
            path,
            image,
            focus: cx.focus_handle(),
            zoom,
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
                let zoom = self.zoom.unwrap();
                div()
                    .w(px(*w as f32 * zoom))
                    .h(px(*h as f32 * zoom))
                    .bg(theme.surface)
                    .child(img(image.clone()).size_full())
                    .into_any_element()
            }
            Ok((image, _, _)) => div()
                .size_full()
                .child(
                    img(image.clone())
                        .size_full()
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
            .flex()
            .flex_col()
            .child(
                div()
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
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("image-one")
                            .cursor_pointer()
                            .child("1:1")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = Some(1.);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("image-out")
                            .cursor_pointer()
                            .child("−")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = Some((this.zoom.unwrap_or(1.) / 1.5).max(0.01));
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("image-in")
                            .cursor_pointer()
                            .child("+")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = Some((this.zoom.unwrap_or(1.) * 1.5).min(16.));
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("image-pan")
                    .flex_1()
                    .min_h_0()
                    .when(self.zoom.is_some(), |d| d.overflow_scroll())
                    .when(self.zoom.is_none(), |d| d.overflow_hidden())
                    .p_6()
                    .child(image),
            )
    }
}
