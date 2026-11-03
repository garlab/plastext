use super::*;
use gpui::{AnyElement, ContentMask, MouseButton, canvas, div, fill, px, rgb, size};
use plastext_editor::Theme;

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        self.listen_for_pinch(window, cx);
        let theme = Theme::get(window, cx);
        let body: AnyElement = if self.show_raw {
            if let Some(raw) = &self.raw_text {
                raw.clone().into_any_element()
            } else {
                self.raw_hex.as_ref().unwrap().clone().into_any_element()
            }
        } else {
            let view = cx.entity();
            div()
                .size_full()
                .relative()
                .overflow_hidden()
                .key_context("ImagePreview")
                .track_focus(&self.focus)
                .cursor(gpui::CursorStyle::OpenHand)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, window, _| {
                        this.focus.focus(window);
                        this.drag = Some(event.position);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.drag = None),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.drag = None),
                )
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.drag = None;
                    }
                    if let Some(previous) = this.drag {
                        this.pan.x += f64::from(previous.x - event.position.x);
                        this.pan.y += f64::from(previous.y - event.position.y);
                        this.drag = Some(event.position);
                        cx.notify();
                    }
                }))
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let delta = event.delta.pixel_delta(px(32.));
                    if event.modifiers.platform || event.modifiers.control {
                        this.zoom_by(
                            (f64::from(delta.y) * 0.01).clamp(-1., 1.).exp(),
                            Some(event.position),
                            cx,
                        );
                    } else {
                        this.pan.x -= f64::from(delta.x);
                        this.pan.y -= f64::from(delta.y);
                        cx.notify();
                    }
                }))
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            view.update(cx, |this, cx| this.layout(bounds, window, cx))
                        },
                        paint,
                    )
                    .size_full(),
                )
                .when(self.info.is_none() || self.error.is_some(), |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_4()
                            .left_4()
                            .right_4()
                            .p_3()
                            .bg(theme.surface)
                            .text_color(theme.text)
                            .child(
                                self.error
                                    .clone()
                                    .unwrap_or_else(|| "Opening image…".into()),
                            ),
                    )
                })
                .into_any_element()
        };
        let animated = self.info.as_ref().is_some_and(|i| i.frames > 1);
        let playing = self
            .info
            .as_ref()
            .is_some_and(|i| self.playback.next(i).is_some());
        div()
            .key_context("Image")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::toggle_source))
            .on_action(cx.listener(Self::toggle_playback))
            .child(
                div()
                    .id("image-controls")
                    .flex_none()
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .overflow_x_scroll()
                    .whitespace_nowrap()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .text_xs()
                    .child(
                        self.info
                            .as_ref()
                            .map_or("Image", |i| i.format.as_str())
                            .to_owned(),
                    )
                    .when(!self.show_raw, |d| {
                        d.child(
                            div()
                                .id("image-fit")
                                .cursor_pointer()
                                .child("Fit")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.zoom = None;
                                    this.pan = point(0., 0.);
                                    this.focus.focus(window);
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("image-one")
                                .cursor_pointer()
                                .child("1:1")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(v) = &this.viewport {
                                        this.zoom_by(1. / v.zoom, None, cx);
                                    }
                                    this.focus.focus(window);
                                })),
                        )
                        .child(
                            div()
                                .id("image-minus")
                                .cursor_pointer()
                                .child("−")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.zoom_by(1. / 1.5, None, cx);
                                    this.focus.focus(window);
                                })),
                        )
                        .child(div().id("image-plus").cursor_pointer().child("+").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.zoom_by(1.5, None, cx);
                                this.focus.focus(window);
                            }),
                        ))
                        .child(format!(
                            "{:.0}%",
                            self.viewport.as_ref().map_or(1., |v| v.zoom) * 100.
                        ))
                        .when(animated, |d| {
                            d.child(
                                div()
                                    .id("image-playback")
                                    .cursor_pointer()
                                    .child(if playing { "Pause" } else { "Play" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_playback(&TogglePlayback, window, cx)
                                    })),
                            )
                            .child(format!(
                                "{} / {}",
                                self.playback.frame + 1,
                                self.info.as_ref().unwrap().frames
                            ))
                        })
                    })
                    .when(self.error.is_some() && !self.show_raw, |d| {
                        d.child(
                            div()
                                .id("image-retry")
                                .cursor_pointer()
                                .child("Retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    this.failed = None;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("image-source")
                            .cursor_pointer()
                            .child(if self.show_raw {
                                "Preview ⇧⌘V"
                            } else if self.lines.is_some() {
                                "Source ⇧⌘V"
                            } else {
                                "Binary ⇧⌘V"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_source(&ToggleSource, window, cx)
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(body))
    }
}

fn paint(bounds: Bounds<Pixels>, layout: Layout, window: &mut Window, _: &mut App) {
    let Some(paper) = layout.paper else {
        return;
    };
    let clip = paper.intersect(&bounds);
    if clip.size.width <= px(0.) || clip.size.height <= px(0.) {
        return;
    }
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        window.paint_quad(fill(clip, rgb(0xd0d0d0)));
        // Bound paint commands even on a very large display.
        let cell =
            px(12f32
                .max((f32::from(clip.size.width) * f32::from(clip.size.height) / 4096.).sqrt()));
        let first_x = ((clip.left() - paper.left()) / cell).floor() as i32;
        let first_y = ((clip.top() - paper.top()) / cell).floor() as i32;
        let cols = (clip.size.width / cell).ceil() as i32 + 1;
        let rows = (clip.size.height / cell).ceil() as i32 + 1;
        for y in first_y..first_y + rows {
            for x in first_x..first_x + cols {
                if (x + y) % 2 == 0 {
                    window.paint_quad(fill(
                        Bounds::new(
                            paper.origin + point(cell * x as f32, cell * y as f32),
                            size(cell, cell),
                        ),
                        rgb(0xeeeeee),
                    ));
                }
            }
        }
        for (index, (rect, image)) in layout.images.iter().enumerate() {
            // A tile replaces the overview beneath it. Blending both images
            // would apply translucent pixels twice and produce dark seams.
            let masks = uncovered(
                rect.intersect(&clip),
                layout.images[index + 1..].iter().map(|(r, _)| *r),
            );
            for mask in masks {
                window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                    let _ = window.paint_image(*rect, Default::default(), image.clone(), 0, false);
                });
            }
        }
        if layout.pixel >= 8. {
            let pixel = px(layout.pixel);
            let start_x = ((clip.left() - paper.left()) / pixel).ceil() as u32;
            let start_y = ((clip.top() - paper.top()) / pixel).ceil() as u32;
            let color = gpui::rgba(0x00000040);
            for x in start_x..start_x + (clip.size.width / pixel).ceil().min(2048.) as u32 {
                window.paint_quad(fill(
                    Bounds::new(
                        point(paper.left() + pixel * x as f32, clip.top()),
                        size(px(1.), clip.size.height),
                    ),
                    color,
                ));
            }
            for y in start_y..start_y + (clip.size.height / pixel).ceil().min(2048.) as u32 {
                window.paint_quad(fill(
                    Bounds::new(
                        point(clip.left(), paper.top() + pixel * y as f32),
                        size(clip.size.width, px(1.)),
                    ),
                    color,
                ));
            }
        }
    });
}

fn uncovered(
    rect: Bounds<Pixels>,
    covers: impl Iterator<Item = Bounds<Pixels>>,
) -> Vec<Bounds<Pixels>> {
    if rect.size.width <= px(0.) || rect.size.height <= px(0.) {
        return Vec::new();
    }
    let mut pieces = vec![rect];
    for cover in covers {
        pieces = pieces
            .into_iter()
            .flat_map(|r| {
                let cut = r.intersect(&cover);
                if cut.size.width <= px(0.) || cut.size.height <= px(0.) {
                    return vec![r];
                }
                [
                    Bounds::new(r.origin, size(r.size.width, cut.top() - r.top())),
                    Bounds::new(
                        point(r.left(), cut.bottom()),
                        size(r.size.width, r.bottom() - cut.bottom()),
                    ),
                    Bounds::new(
                        point(r.left(), cut.top()),
                        size(cut.left() - r.left(), cut.size.height),
                    ),
                    Bounds::new(
                        point(cut.right(), cut.top()),
                        size(r.right() - cut.right(), cut.size.height),
                    ),
                ]
                .into_iter()
                .filter(|r| r.size.width > px(0.) && r.size.height > px(0.))
                .collect()
            })
            .collect();
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translucent_overview_and_tiles_paint_each_pixel_exactly_once() {
        let rect = |x, y, w, h| Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        let images = [
            rect(0., 0., 32., 32.),
            rect(8., 8., 16., 16.),
            rect(16., 0., 16., 24.),
        ];
        let painted: Vec<_> = images
            .iter()
            .enumerate()
            .flat_map(|(i, r)| uncovered(*r, images[i + 1..].iter().copied()))
            .collect();
        for y in 0..32 {
            for x in 0..32 {
                let p = point(px(x as f32 + 0.5), px(y as f32 + 0.5));
                assert_eq!(
                    painted.iter().filter(|r| r.contains(&p)).count(),
                    1,
                    "{x},{y}"
                );
            }
        }
        assert!(uncovered(images[0], std::iter::once(images[0])).is_empty());
    }
}
