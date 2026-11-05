mod model;
use crate::pdf_view::PdfView;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, Pixels, Point, Render, SharedString,
    Subscription, Task, Window, actions, div, prelude::*, px,
};
use model::Model;
use plastext_buffer::{FileSource, LineIndex, OpenOptions};
use plastext_editor::{TextStyle, TextView, Theme};
use plastext_syntax::{Language, Symbol};
use std::{path::PathBuf, sync::Arc};

actions!(tex_view, [ToggleSource, Rebuild]);
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-shift-v", ToggleSource, Some("Tex")),
        KeyBinding::new("cmd-r", Rebuild, Some("Tex")),
    ]);
}
pub struct TexView {
    path: PathBuf,
    style: TextStyle,
    source: Entity<TextView>,
    source_observer: Subscription,
    model: Entity<Model>,
    _model_observer: Subscription,
    pdf: Option<Entity<PdfView>>,
    pdf_observer: Option<Subscription>,
    staging: Option<Entity<PdfView>>,
    staging_observer: Option<Subscription>,
    staged_reading: Option<(u32, bool, Point<f64>)>,
    prepare: Option<Task<()>>,
    source_reload: Option<Task<()>>,
    output_id: u64,
    preview: bool,
    visible: bool,
    zoom: f32,
    focus: FocusHandle,
    old_focus: Option<FocusHandle>,
    error: Option<SharedString>,
    details: bool,
    settings_open: bool,
}
impl TexView {
    pub fn new(
        path: PathBuf,
        source: Arc<FileSource>,
        index: Arc<LineIndex>,
        style: TextStyle,
        language: Option<&'static Language>,
        config: (Option<PathBuf>, plastext_settings::Tex),
        cx: &mut Context<Self>,
    ) -> Self {
        let source = cx.new(|cx| {
            let mut view = TextView::new(source, index, style.clone(), language, cx);
            view.set_file_path(&path);
            view
        });
        let preview = config.1.preview;
        let model = cx.new(|_| Model::new(path.clone(), config.0, config.1));
        let mut view = Self::with_model(path, source, model, style, cx);
        view.preview = preview;
        view
    }
    fn with_model(
        path: PathBuf,
        source: Entity<TextView>,
        model: Entity<Model>,
        style: TextStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let source_observer = cx.observe(&source, |_, _, cx| cx.notify());
        let model_observer = cx.observe(&model, |this, _, cx| {
            this.prepare_output(cx);
            cx.notify();
        });
        let owner = cx.entity_id();
        cx.on_release(move |this, cx| {
            this.model
                .update(cx, |model, _| model.visible(owner, false))
        })
        .detach();
        Self {
            path,
            style,
            source,
            source_observer,
            model,
            _model_observer: model_observer,
            pdf: None,
            pdf_observer: None,
            staging: None,
            staging_observer: None,
            staged_reading: None,
            prepare: None,
            source_reload: None,
            output_id: 0,
            preview: false,
            visible: false,
            zoom: 1.,
            focus: cx.focus_handle(),
            old_focus: None,
            error: None,
            details: false,
            settings_open: false,
        }
    }
    pub fn duplicate(&self, cx: &mut Context<Self>) -> Entity<Self> {
        let source = self.source.update(cx, |view, cx| view.duplicate(cx));
        cx.new(|cx| {
            let mut view = Self::with_model(
                self.path.clone(),
                source,
                self.model.clone(),
                self.style.clone(),
                cx,
            );
            view.preview = self.preview;
            view.zoom = self.zoom;
            if let Some(pdf) = &self.pdf {
                view.pdf = Some(pdf.update(cx, |pdf, cx| pdf.duplicate(cx)));
                view.pdf_observer =
                    Some(cx.observe(view.pdf.as_ref().unwrap(), |_, _, cx| cx.notify()));
                view.output_id = self.output_id;
            }
            view
        })
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        let active = visible && self.preview;
        let owner = cx.entity_id();
        self.model.update(cx, |model, cx| {
            model.visible(owner, active);
            if active {
                model.setup(cx);
            }
        });
        if let Some(pdf) = &self.pdf {
            pdf.update(cx, |pdf, cx| pdf.set_visible(active, cx));
        }
        // Finish visible resizing/scrolling before rasterizing its replacement;
        // both then fit the existing displayed/replacement cache allowance.
        let can_stage = self
            .pdf
            .as_ref()
            .is_none_or(|pdf| pdf.read(cx).ready() || pdf.read(cx).notice().is_some());
        if let Some(pdf) = &self.staging {
            pdf.update(cx, |pdf, cx| pdf.set_visible(active && can_stage, cx));
        }
        if active {
            self.prepare_output(cx);
        }
    }
    pub fn set_zoom(&mut self, factor: f32, font_size: Pixels, cx: &mut Context<Self>) {
        self.zoom = factor;
        self.style.font_size = font_size;
        self.source
            .update(cx, |source, cx| source.set_font_size(font_size, cx));
        for pdf in [&self.pdf, &self.staging].into_iter().flatten() {
            pdf.update(cx, |pdf, cx| pdf.set_zoom(factor, font_size, cx));
        }
        cx.notify();
    }
    pub fn symbols(&self, cx: &App) -> Option<Arc<Vec<Symbol>>> {
        self.source.read(cx).symbols()
    }
    pub fn position(&self, cx: &App) -> u64 {
        self.source.read(cx).position()
    }
    pub fn scroll_to_offset(&mut self, offset: u64, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.scroll_to_offset(offset, cx));
    }
    pub fn scroll_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.scroll_to_line(line, cx));
    }
    pub fn notice(&self, cx: &App) -> Option<SharedString> {
        self.error
            .clone()
            .or_else(|| self.model.read(cx).state.error.clone().map(Into::into))
    }
    pub fn summary(&self, cx: &App) -> String {
        if !self.preview {
            return "TeX · Source".into();
        }
        let model = self.model.read(cx);
        format!(
            "TeX · {}{}",
            model.state.status,
            self.pdf
                .as_ref()
                .map(|pdf| format!(" · {}", pdf.read(cx).summary(cx)))
                .unwrap_or_default()
        )
    }
    fn toggle(&mut self, _: &ToggleSource, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = !self.preview;
        if self.preview {
            self.model.update(cx, |model, cx| model.setup(cx));
        } else {
            let path = self.path.clone();
            self.source_reload = Some(cx.spawn_in(window, async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { plastext_buffer::open(&path, OpenOptions::default()) })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Ok((source, index)) = result {
                        let position = this.source.read(cx).position();
                        this.source = cx.new(|cx| {
                            let mut view = TextView::new(
                                source,
                                index,
                                this.style.clone(),
                                plastext_syntax::detect(&this.path, b""),
                                cx,
                            );
                            view.set_file_path(&this.path);
                            view.scroll_to_offset(position, cx);
                            view
                        });
                        this.source_observer = cx.observe(&this.source, |_, _, cx| cx.notify());
                        if !this.preview {
                            this.source.focus_handle(cx).focus(window);
                        }
                    }
                    cx.notify();
                });
            }));
        }
        self.set_visible(self.visible, cx);
        self.focus_handle(cx).focus(window);
        cx.notify();
    }
    fn prepare_output(&mut self, cx: &mut Context<Self>) {
        if !self.visible || !self.preview {
            return;
        }
        let Some(output) = self.model.read(cx).state.output.clone() else {
            // A root change applies to every split sharing this model. Never
            // retain another root's PDF as the new document's fallback.
            self.prepare = None;
            if let Some(old) = self.pdf.take() {
                self.old_focus = Some(old.focus_handle(cx));
                old.update(cx, |pdf, cx| pdf.set_visible(false, cx));
            }
            if let Some(old) = self.staging.take() {
                old.update(cx, |pdf, cx| pdf.set_visible(false, cx));
            }
            self.pdf_observer = None;
            self.staging_observer = None;
            self.output_id = 0;
            return;
        };
        let id = output.document.id;
        if self.output_id == id {
            return;
        }
        self.output_id = id;
        self.error = None;
        self.prepare = Some(cx.spawn(async move |this, cx| {
            let document = output.document.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    FileSource::from_file(
                        document.file()?,
                        OpenOptions {
                            large_file_threshold: 0,
                            block_cache: 8 * 1024 * 1024,
                        },
                    )
                    .map(Arc::new)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.output_id != id {
                    return;
                }
                let source = match result {
                    Ok(source) => source,
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        return;
                    }
                };
                if let Some(old) = this.staging.take() {
                    old.update(cx, |pdf, cx| pdf.set_visible(false, cx));
                }
                this.staged_reading = this.pdf.as_ref().map(|pdf| pdf.read(cx).reading_state());
                if let Some(old) = &this.pdf {
                    old.update(cx, |pdf, cx| pdf.suspend_prefetch(true, cx));
                }
                let pdf = cx.new(|cx| {
                    let mut pdf = PdfView::compiled(
                        this.path.clone(),
                        source,
                        &output,
                        this.style.clone(),
                        cx,
                    );
                    pdf.stage(this.pdf.is_some());
                    pdf.set_zoom(this.zoom, this.style.font_size, cx);
                    if let Some(state) = this.staged_reading {
                        pdf.restore_reading_state(state, cx);
                    }
                    pdf.set_visible(this.visible && this.preview, cx);
                    pdf
                });
                this.staging_observer = Some(cx.observe(&pdf, |this, _, cx| {
                    this.present_if_ready(cx);
                    cx.notify();
                }));
                this.staging = Some(pdf);
                cx.notify();
            });
        }));
    }
    fn present_if_ready(&mut self, cx: &mut Context<Self>) {
        let Some(staged) = self.staging.clone() else {
            return;
        };
        let state = self.pdf.as_ref().map(|pdf| pdf.read(cx).reading_state());
        if self.staged_reading != state {
            self.staged_reading = state;
            if let Some(state) = state {
                staged.update(cx, |pdf, cx| pdf.restore_reading_state(state, cx));
            }
        }
        if let Some(error) = staged.read(cx).notice() {
            self.error = Some(error);
            staged.update(cx, |pdf, cx| pdf.set_visible(false, cx));
            self.staging = None;
            self.staging_observer = None;
            if let Some(old) = &self.pdf {
                old.update(cx, |pdf, cx| pdf.suspend_prefetch(false, cx));
            }
            return;
        }
        if !staged.read(cx).ready() {
            return;
        }
        if let Some(old) = self.pdf.take() {
            self.old_focus = Some(old.focus_handle(cx));
            old.update(cx, |pdf, cx| pdf.set_visible(false, cx));
        }
        self.pdf = self.staging.take();
        staged.update(cx, |pdf, cx| pdf.publish(cx));
        self.staging_observer = None;
        self.pdf_observer = Some(cx.observe(&staged, |this, _, cx| {
            this.present_if_ready(cx);
            cx.notify();
        }));
        cx.notify();
    }
    fn pick(&mut self, root: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if root {
                    "Choose TeX root document"
                } else {
                    "Choose pdfLaTeX"
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    if root {
                        this.prepare = None;
                        for pdf in [this.pdf.take(), this.staging.take()].into_iter().flatten() {
                            pdf.update(cx, |pdf, cx| pdf.set_visible(false, cx));
                        }
                        this.pdf_observer = None;
                        this.staging_observer = None;
                        this.output_id = 0;
                        this.error = None;
                    }
                    this.model.update(cx, |model, cx| {
                        if root {
                            model.choose_root(path, cx);
                        } else {
                            model.select_engine(path, cx);
                        }
                    });
                    this.focus_handle(cx).focus(window);
                });
            }
        })
        .detach();
    }
    fn add_input_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Allow TeX input folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.model
                        .update(cx, |model, cx| model.add_input_folder(path, cx));
                    this.focus_handle(cx).focus(window);
                });
            }
        })
        .detach();
    }
}
impl Focusable for TexView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if !self.preview {
            self.source.focus_handle(cx)
        } else {
            self.pdf
                .as_ref()
                .map_or_else(|| self.focus.clone(), |pdf| pdf.focus_handle(cx))
        }
    }
}
impl Render for TexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.preview
            && (self
                .old_focus
                .take()
                .is_some_and(|focus| focus.is_focused(window))
                || self.focus.is_focused(window))
        {
            self.focus_handle(cx).focus(window);
        }
        let theme = Theme::get(window, cx);
        let model = self.model.read(cx);
        let input = model.input.clone();
        let status = model.state.status.clone();
        let slow = model.slow();
        let cancelled = status == "Preview cancelled";
        let error = self.notice(cx);
        let body = if !self.preview {
            self.source.clone().into_any_element()
        } else {
            div()
                .relative()
                .size_full()
                .track_focus(&self.focus)
                .when_some(self.staging.clone(), |d, staged| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(if self.pdf.is_some() { 0. } else { 1. })
                            .child(staged),
                    )
                })
                .when_some(self.pdf.clone(), |d, pdf| {
                    d.child(div().relative().size_full().occlude().child(pdf))
                })
                .when(self.pdf.is_none() && self.staging.is_none(), |d| {
                    d.child(
                        div()
                            .p_6()
                            .text_color(theme.text_muted)
                            .child(status.clone()),
                    )
                })
                .when_some(
                    error.clone().filter(|_| self.details || self.pdf.is_none()),
                    |d, error| {
                        d.child(
                            div()
                                .id("tex-error")
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .max_h(px(170.))
                                .overflow_y_scroll()
                                .p_3()
                                .bg(theme.surface)
                                .text_color(theme.text)
                                .text_sm()
                                .child(error),
                        )
                    },
                )
                .into_any_element()
        };
        div()
            .key_context("Tex")
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::toggle))
            .on_action(cx.listener(|this, _: &Rebuild, _, cx| {
                this.model.update(cx, |model, cx| model.rebuild(cx))
            }))
            .child(
                div()
                    .min_h(px(30.))
                    .flex_none()
                    .px_3()
                    .py_1()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_3()
                    .gap_y_1()
                    .whitespace_nowrap()
                    .bg(theme.surface)
                    .text_color(theme.text_muted)
                    .text_xs()
                    .child("TeX")
                    .child(div().flex_1().min_w_0().truncate().child(if self.preview {
                        status
                    } else {
                        String::new()
                    }))
                    .when(self.preview && slow, |bar| {
                        bar.child(
                            div()
                                .id("tex-cancel")
                                .cursor_pointer()
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.model.update(cx, |model, _| model.cancel());
                                    this.focus_handle(cx).focus(window);
                                })),
                        )
                    })
                    .when(self.preview && (error.is_some() || cancelled), |bar| {
                        bar.child(
                            div()
                                .id("tex-retry")
                                .cursor_pointer()
                                .child("Retry")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.model.update(cx, |model, cx| model.rebuild(cx));
                                    this.focus_handle(cx).focus(window);
                                })),
                        )
                    })
                    .when(
                        self.preview && error.is_some() && self.pdf.is_some(),
                        |bar| {
                            bar.child(
                                div()
                                    .id("tex-details")
                                    .cursor_pointer()
                                    .child("Details")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.details = !this.details;
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .when(self.preview, |bar| {
                        bar.child(
                            div()
                                .id("tex-settings")
                                .cursor_pointer()
                                .child("Settings…")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings_open = !this.settings_open;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("tex-toggle")
                            .cursor_pointer()
                            .child(if self.preview {
                                "Source  ⇧⌘V"
                            } else {
                                "Preview  ⇧⌘V"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle(&ToggleSource, window, cx)
                            })),
                    ),
            )
            .when(self.preview && self.settings_open, |d| {
                d.child(
                    div()
                        .flex_none()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .bg(theme.surface)
                        .text_color(theme.text_muted)
                        .text_sm()
                        .child(format!("Root: {}", input.display()))
                        .child(
                            "Project overrides: .plastext/tex.toml. Retry after changing settings.",
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .child(
                                    div()
                                        .id("tex-root")
                                        .cursor_pointer()
                                        .child("Choose root…")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick(true, window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id("tex-engine")
                                        .cursor_pointer()
                                        .child("Choose pdfLaTeX…")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick(false, window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id("tex-inputs")
                                        .cursor_pointer()
                                        .child("Add input folder…")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.add_input_folder(window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id("tex-settings-retry")
                                        .cursor_pointer()
                                        .child("Retry")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.model.update(cx, |model, cx| model.rebuild(cx));
                                            this.focus_handle(cx).focus(window);
                                        })),
                                ),
                        ),
                )
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}
