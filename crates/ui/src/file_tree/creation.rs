use super::*;
use crate::workspace::file_actions::name_error;
use plastext_editor::{TextInput, TextInputEvent};

pub(super) struct Creation {
    parent: PathBuf,
    folder: bool,
    pub input: Entity<TextInput>,
    error: Option<SharedString>,
    submitted: Option<PathBuf>,
    _changes: Subscription,
}

impl FileTreeView {
    pub(crate) fn begin_create(
        &mut self,
        parent: PathBuf,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .creation
            .as_ref()
            .is_some_and(|creation| creation.submitted.is_some())
        {
            return;
        }
        if !parent.starts_with(self.tree.root()) {
            return;
        }
        if let Some(creation) = &self.creation
            && creation.parent == parent
            && creation.folder == folder
        {
            creation.input.focus_handle(cx).focus(window);
            return;
        }
        for dir in parent
            .ancestors()
            .take_while(|dir| dir.starts_with(self.tree.root()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            self.tree.expand(dir);
            self.list(dir.to_owned(), cx);
        }
        let input =
            cx.new(|cx| TextInput::new(if folder { "Folder name" } else { "Filename" }, cx));
        let changes = cx.subscribe(&input, |this, input, TextInputEvent::Changed, cx| {
            if let Some(creation) = &mut this.creation {
                let text = input.read(cx).text();
                creation.error = if text.trim().is_empty() {
                    None
                } else {
                    name_error(text, creation.folder).map(Into::into)
                };
            }
            cx.notify();
        });
        input.focus_handle(cx).focus(window);
        self.creation = Some(Creation {
            parent,
            folder,
            input,
            error: None,
            submitted: None,
            _changes: changes,
        });
        self.scroll_handle
            .scroll_to_item(self.creation_position().0, gpui::ScrollStrategy::Center);
        cx.notify();
    }
    pub(crate) fn is_creating(&self, path: &std::path::Path) -> bool {
        self.creation
            .as_ref()
            .and_then(|creation| creation.submitted.as_deref())
            == Some(path)
    }
    pub(crate) fn finish_create(
        &mut self,
        path: &std::path::Path,
        error: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if !self.is_creating(path) {
            return;
        }
        if let Some(error) = error {
            let creation = self.creation.as_mut().unwrap();
            creation.error = Some(error);
            creation.submitted = None;
        } else {
            self.creation = None;
        }
        cx.notify();
    }
    pub(super) fn cancel_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .creation
            .as_ref()
            .is_some_and(|creation| creation.submitted.is_some())
        {
            return;
        }
        self.creation = None;
        self.focus.focus(window);
        cx.notify();
    }
    pub(super) fn submit_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(creation) = &mut self.creation else {
            return;
        };
        if creation.submitted.is_some() {
            return;
        }
        let name = creation.input.read(cx).text();
        if name.trim().is_empty() {
            self.cancel_create(window, cx);
            return;
        }
        if let Some(error) = name_error(name, creation.folder) {
            creation.error = Some(error.into());
        }
        if creation.error.is_some() {
            cx.notify();
            return;
        }
        let path = creation.parent.join(name);
        creation.submitted = Some(path.clone());
        cx.emit(FileTreeEvent::Create {
            path,
            folder: creation.folder,
        });
        cx.notify();
    }
    pub(super) fn creation_position(&self) -> (usize, usize) {
        self.creation
            .as_ref()
            .and_then(|creation| {
                if creation.parent == self.tree.root() {
                    return Some((0, 0));
                }
                self.tree
                    .rows()
                    .iter()
                    .enumerate()
                    .find(|(_, row)| row.entry.path == creation.parent)
                    .map(|(index, row)| (index + 1, row.depth + 1))
            })
            .unwrap_or((0, 0))
    }
    pub(super) fn creation_rows(&self) -> usize {
        self.creation
            .as_ref()
            .map_or(0, |creation| 1 + usize::from(creation.error.is_some()))
    }
    pub(super) fn render_creation(
        &self,
        error: bool,
        depth: usize,
        theme: Theme,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let creation = self.creation.as_ref().unwrap();
        let row = div()
            .h(px(ROW_HEIGHT))
            .pl(px(8. + INDENT * depth as f32))
            .pr_2()
            .flex()
            .items_center()
            .gap_1();
        if error {
            return row
                .id("tree-create-error")
                .debug_selector(|| "tree-create-error".into())
                .text_xs()
                .text_color(gpui::rgb(0xe06c75))
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(creation.error.clone().unwrap()),
                )
                .into_any_element();
        }
        let input = creation.input.clone();
        row.id("tree-create-input")
            .debug_selector(|| "tree-create-input".into())
            .child(icon(if creation.folder { 1 } else { 0 }, theme.text_muted))
            .child(div().flex_1().min_w_0().child(input.clone()))
            .capture_key_down(cx.listener(|this, _, window, cx| {
                if this
                    .creation
                    .as_ref()
                    .is_some_and(|creation| creation.submitted.is_some())
                {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                input.focus_handle(cx).focus(window);
                cx.stop_propagation();
            })
            .into_any_element()
    }
    pub(super) fn render_toolbar(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        div()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .justify_end()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .children(
                [
                    (0, "tree-new-file", "New File"),
                    (1, "tree-new-folder", "New Folder"),
                    (2, "tree-collapse-all", "Collapse All Folders"),
                ]
                .into_iter()
                .map(|(kind, id, label)| {
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .size(px(24.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|button| button.bg(theme.border))
                        .tooltip(move |_, cx| cx.new(|_| Tooltip(label)).into())
                        .child(icon(kind, theme.text_muted))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            if kind == 2 {
                                if this
                                    .creation
                                    .as_ref()
                                    .is_some_and(|creation| creation.submitted.is_some())
                                {
                                    return;
                                }
                                this.cancel_create(window, cx);
                                this.tree.collapse_all();
                                if let Some(path) = &this.selected {
                                    this.selected = this
                                        .tree
                                        .rows()
                                        .iter()
                                        .find(|row| path.starts_with(&row.entry.path))
                                        .map(|row| row.entry.path.clone());
                                }
                                this.scroll_handle
                                    .scroll_to_item(0, gpui::ScrollStrategy::Top);
                                cx.notify();
                            } else {
                                cx.emit(FileTreeEvent::New { folder: kind == 1 });
                            }
                        }))
                }),
            )
    }
}

struct Tooltip(&'static str);
impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(window, cx);
        div()
            .px_2()
            .py_1()
            .text_xs()
            .bg(theme.surface)
            .text_color(theme.text)
            .border_1()
            .border_color(theme.border)
            .rounded_sm()
            .child(self.0)
    }
}

fn icon(kind: u8, color: gpui::Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let strokes: &[&[(f32, f32)]] = match kind {
                0 => &[
                    &[
                        (10., 16.),
                        (3., 16.),
                        (3., 2.),
                        (10., 2.),
                        (14., 6.),
                        (14., 9.),
                    ],
                    &[(10., 2.), (10., 6.), (14., 6.)],
                    &[(13., 11.), (13., 17.)],
                    &[(10., 14.), (16., 14.)],
                ],
                1 => &[
                    &[
                        (9., 15.),
                        (2., 15.),
                        (2., 3.),
                        (7., 3.),
                        (9., 5.),
                        (16., 5.),
                        (16., 10.),
                    ],
                    &[(13., 11.), (13., 17.)],
                    &[(10., 14.), (16., 14.)],
                ],
                _ => &[
                    &[(5., 2.), (16., 2.), (16., 13.)],
                    &[(2., 5.), (13., 5.), (13., 16.), (2., 16.), (2., 5.)],
                    &[(5., 10.5), (10., 10.5)],
                ],
            };
            let mut path = PathBuilder::stroke(px(1.3));
            for stroke in strokes {
                path.move_to(bounds.origin + point(px(stroke[0].0), px(stroke[0].1)));
                for &(x, y) in &stroke[1..] {
                    path.line_to(bounds.origin + point(px(x), px(y)));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(18.))
    .flex_none()
}
