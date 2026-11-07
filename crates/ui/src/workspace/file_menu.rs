use super::*;
use crate::file_tree::FileAction;
use gpui::{ClipboardItem, KeyDownEvent, anchored};

pub(super) struct FileMenu {
    path: PathBuf,
    position: Point<Pixels>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    selected: Option<usize>,
    background: bool,
}

#[derive(Clone, Copy)]
enum CopyPath {
    Absolute,
    Relative,
}
#[derive(Clone, Copy)]
enum Command {
    File(FileAction),
    Path(CopyPath),
    CreateFile,
    CreateFolder,
    PasteInto,
}
const ITEMS: [(Command, &str, &str); 9] = [
    (Command::File(FileAction::Duplicate), "Duplicate", ""),
    (Command::File(FileAction::Copy), "Copy", "⌘C"),
    (Command::File(FileAction::Cut), "Cut", "⌘X"),
    (Command::File(FileAction::Paste), "Paste", "⌘V"),
    (Command::File(FileAction::Rename), "Rename…", "F2"),
    (Command::File(FileAction::Move), "Move…", ""),
    (Command::File(FileAction::Trash), "Delete", "⌘⌫"),
    (Command::Path(CopyPath::Absolute), "Copy Path", ""),
    (Command::Path(CopyPath::Relative), "Copy Relative Path", ""),
];

const BACKGROUND_ITEMS: [(Command, &str, &str); 3] = [
    (Command::CreateFile, "New File", "⌘N"),
    (Command::CreateFolder, "New Folder", ""),
    (Command::PasteInto, "Paste File", "⌘V"),
];

impl FileMenu {
    fn items(&self) -> &'static [(Command, &'static str, &'static str)] {
        if self.background {
            &BACKGROUND_ITEMS
        } else {
            &ITEMS
        }
    }
}

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
        path: Option<PathBuf>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let background = path.is_none();
        let Some(path) = path.or_else(|| self.root.clone()) else {
            return;
        };
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
            background,
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

    fn file_menu_enabled(&self, command: Command, cx: &App) -> bool {
        match command {
            Command::Path(_) => true,
            Command::File(FileAction::Paste) | Command::PasteInto => {
                !self.file_busy && !self.close_prompt && Self::can_paste_file(cx)
            }
            _ => !self.file_busy && !self.close_prompt,
        }
    }
    fn file_menu_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        if !self.file_menu_enabled(command, cx) {
            return;
        }
        let Some(menu) = &self.file_menu else { return };
        let path = menu.path.clone();
        self.dismiss_file_menu(window, cx);
        match command {
            Command::Path(action) => cx.write_to_clipboard(ClipboardItem::new_string(
                action.text(&path, self.root.as_deref()),
            )),
            Command::File(action) => self.file_action_at(action, path, window, cx),
            Command::CreateFile => self.new_tree_item(Some(path), false, window, cx),
            Command::CreateFolder => self.new_tree_item(Some(path), true, window, cx),
            Command::PasteInto => self.paste_file_into(path, window, cx),
        }
    }

    fn file_menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.prevent_default();
        if event.keystroke.modifiers.platform && event.keystroke.key == "n" {
            self.new_file(&NewFile, window, cx);
            return;
        }
        let Some(menu) = &self.file_menu else { return };
        let items = menu.items();
        if event.keystroke.modifiers.platform {
            if menu.background {
                if event.keystroke.key == "v" {
                    self.file_menu_command(Command::PasteInto, window, cx);
                }
                return;
            }
            let index = match event.keystroke.key.as_str() {
                "c" => 1,
                "x" => 2,
                "v" => 3,
                "backspace" => 6,
                _ => return,
            };
            self.file_menu_command(ITEMS[index].0, window, cx);
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_file_menu(window, cx),
            "enter" | "space" => {
                if let Some(index) = self.file_menu.as_ref().and_then(|menu| menu.selected) {
                    self.file_menu_command(items[index].0, window, cx);
                }
            }
            "up" | "down" => {
                let enabled: Vec<_> = items
                    .iter()
                    .map(|(command, _, _)| self.file_menu_enabled(*command, cx))
                    .collect();
                if let Some(menu) = &mut self.file_menu {
                    let down = event.keystroke.key == "down";
                    let count = items.len();
                    let current = menu.selected.unwrap_or(if down { count - 1 } else { 0 });
                    menu.selected = (1..=count)
                        .map(|step| {
                            if down {
                                (current + step) % count
                            } else {
                                (current + count - step) % count
                            }
                        })
                        .find(|&index| enabled[index]);
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
        let background = menu.background;
        Some(
            anchored()
                .position(menu.position)
                .snap_to_window_with_margin(px(4.))
                .child(
                    div()
                        .id("file-context-menu")
                        .track_focus(&menu.focus)
                        .occlude()
                        .w(px(240.))
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
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down_out(
                            cx.listener(|this, _, window, cx| this.dismiss_file_menu(window, cx)),
                        )
                        .on_key_down(cx.listener(Self::file_menu_key))
                        .children(menu.items().iter().copied().enumerate().map(
                            |(index, (action, label, shortcut))| {
                                let enabled = self.file_menu_enabled(action, cx);
                                div()
                                    .id(("file-menu-item", index))
                                    .px_3()
                                    .py_1()
                                    .rounded_sm()
                                    .when(
                                        if background {
                                            index == 2
                                        } else {
                                            index == 4 || index == 7
                                        },
                                        |item| item.mt_1().border_t_1().border_color(theme.border),
                                    )
                                    .flex()
                                    .justify_between()
                                    .child(label)
                                    .child(div().text_color(theme.text_muted).child(shortcut))
                                    .when(!enabled, |item| item.opacity(0.5))
                                    .cursor_pointer()
                                    .when(selected == Some(index), |item| item.bg(theme.border))
                                    .hover(|item| item.bg(theme.border))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.file_menu_command(action, window, cx);
                                    }))
                            },
                        )),
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
