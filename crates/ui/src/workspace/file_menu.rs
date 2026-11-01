use super::*;
use gpui::{ClipboardItem, KeyDownEvent, anchored};

pub(super) struct FileMenu {
    path: PathBuf,
    position: Point<Pixels>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    selected: Option<usize>,
}

#[derive(Clone, Copy)]
enum CopyPath {
    Absolute,
    Relative,
}
const ITEMS: [(CopyPath, &str); 2] = [
    (CopyPath::Absolute, "Copy Path"),
    (CopyPath::Relative, "Copy Relative Path"),
];

impl CopyPath {
    fn text(self, path: &Path, root: Option<&Path>) -> String {
        let path = match self {
            Self::Absolute => path,
            Self::Relative => root
                .and_then(|root| path.strip_prefix(root).ok())
                .unwrap_or(path),
        };
        path.to_string_lossy().into_owned()
    }
}

impl Workspace {
    pub(super) fn open_file_menu(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_tab_menu(window, cx);
        self.dismiss_file_menu(window, cx);
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        self.file_menu = Some(FileMenu {
            path,
            position,
            focus,
            previous_focus,
            selected: None,
        });
        cx.notify();
    }

    pub(super) fn dismiss_file_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.file_menu.take() {
            if menu.focus.is_focused(window)
                && let Some(previous) = menu.previous_focus
            {
                previous.focus(window);
            }
            cx.notify();
        }
    }

    fn copy_file_path(&mut self, action: CopyPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.file_menu else { return };
        // Capture the right-clicked path, not the active tab or a mutable tree row index.
        let text = action.text(&menu.path, self.root.as_deref());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.dismiss_file_menu(window, cx);
    }

    fn file_menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.prevent_default();
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_file_menu(window, cx),
            "enter" | "space" => {
                if let Some(index) = self.file_menu.as_ref().and_then(|menu| menu.selected) {
                    self.copy_file_path(ITEMS[index].0, window, cx);
                }
            }
            "up" | "down" => {
                if let Some(menu) = &mut self.file_menu {
                    menu.selected = Some(
                        menu.selected
                            .map_or(if event.keystroke.key == "down" { 0 } else { 1 }, |index| {
                                1 - index
                            }),
                    );
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_file_menu(
        &self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.file_menu.as_ref()?;
        let selected = menu.selected;
        Some(
            anchored()
                .position(menu.position)
                .snap_to_window_with_margin(px(4.))
                .child(
                    div()
                        .id("file-context-menu")
                        .track_focus(&menu.focus)
                        .occlude()
                        .w(px(200.))
                        .p_1()
                        .flex()
                        .flex_col()
                        .bg(theme.surface)
                        .text_color(theme.text)
                        .text_sm()
                        .border_1()
                        .border_color(theme.border)
                        .rounded_md()
                        .shadow_lg()
                        .on_mouse_down_out(
                            cx.listener(|this, _, window, cx| this.dismiss_file_menu(window, cx)),
                        )
                        .on_key_down(cx.listener(Self::file_menu_key))
                        .children(
                            ITEMS
                                .into_iter()
                                .enumerate()
                                .map(|(index, (action, label))| {
                                    div()
                                        .id(("file-menu-item", index))
                                        .px_3()
                                        .py_1()
                                        .rounded_sm()
                                        .child(label)
                                        .cursor_pointer()
                                        .when(selected == Some(index), |item| item.bg(theme.border))
                                        .hover(|item| item.bg(theme.border))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.copy_file_path(action, window, cx);
                                        }))
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_paths_preserve_spaces_unicode_and_project_relative_directories() {
        let root = Path::new("/project");
        let file = root.join("notes/Café study.md");
        assert_eq!(
            CopyPath::Absolute.text(&file, Some(root)),
            "/project/notes/Café study.md"
        );
        assert_eq!(
            CopyPath::Relative.text(&file, Some(root)),
            "notes/Café study.md"
        );
        assert_eq!(
            CopyPath::Relative.text(&root.join("README.md"), Some(root)),
            "README.md"
        );
        assert_eq!(
            CopyPath::Relative.text(&root.join("notes"), Some(root)),
            "notes"
        );
    }

    #[test]
    fn relative_path_uses_path_components_not_a_string_prefix() {
        let path = Path::new("/project-other/file.md");
        assert_eq!(
            CopyPath::Relative.text(path, Some(Path::new("/project"))),
            "/project-other/file.md"
        );
        assert_eq!(
            CopyPath::Relative.text(path, None),
            "/project-other/file.md"
        );
    }
}
