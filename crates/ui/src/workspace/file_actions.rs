use super::*;
use crate::file_tree::FileAction;
use gpui::{ClipboardItem, Global, KeyDownEvent};
use plastext_editor::{EditingDocument, TextInput, TextInputEvent};
use plastext_platform::files;

#[derive(Clone)]
pub(super) enum OperationKind {
    CreateFile,
    CreateFolder,
    Duplicate,
    CopyInto(PathBuf),
    MoveTo(PathBuf),
    Trash,
}
#[derive(Clone)]
pub(super) struct Operation {
    pub source: PathBuf,
    pub kind: OperationKind,
}
impl Operation {
    fn changes_source(&self) -> bool {
        matches!(self.kind, OperationKind::MoveTo(_) | OperationKind::Trash)
    }
    fn is_creation(&self) -> bool {
        matches!(
            self.kind,
            OperationKind::CreateFile | OperationKind::CreateFolder
        )
    }
    fn run(&self) -> std::io::Result<Option<PathBuf>> {
        match &self.kind {
            OperationKind::CreateFile => {
                files::create_file(&self.source).map(|()| Some(self.source.clone()))
            }
            OperationKind::CreateFolder => {
                files::create_folder(&self.source).map(|()| Some(self.source.clone()))
            }
            OperationKind::Duplicate => files::duplicate(&self.source).map(Some),
            OperationKind::CopyInto(directory) => {
                files::copy_into(&self.source, directory).map(Some)
            }
            OperationKind::MoveTo(destination) => {
                files::move_to(&self.source, destination).map(|()| Some(destination.clone()))
            }
            OperationKind::Trash => files::trash(&self.source).map(|_| None),
        }
    }
}

struct FileClipboard {
    item: ClipboardItem,
    path: PathBuf,
    cut: bool,
}
impl Global for FileClipboard {}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptKind {
    Rename,
    NewFile,
    NewFolder,
}
impl PromptKind {
    fn title(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::NewFile => "New File",
            Self::NewFolder => "New Folder",
        }
    }
    fn confirm(self) -> &'static str {
        if self == Self::Rename {
            "Rename"
        } else {
            "Create"
        }
    }
    fn error(self, name: &str) -> Option<&'static str> {
        name_error(name, self == Self::NewFolder)
    }
}

pub(super) struct FilePrompt {
    path: PathBuf,
    kind: PromptKind,
    error: Option<SharedString>,
    input: Entity<TextInput>,
    _input_changes: Subscription,
}

fn rename_selection(name: &str, directory: bool) -> std::ops::Range<usize> {
    let end = if directory {
        name.len()
    } else {
        Path::new(name)
            .file_stem()
            .map_or(name.len(), |stem| stem.len())
    };
    0..end
}

fn name_error(name: &str, folder: bool) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some(if folder {
            "Enter a folder name."
        } else {
            "Enter a filename."
        })
    } else if name.contains('/') {
        Some(if folder {
            "Folder names cannot contain a slash (/)."
        } else {
            "Filenames cannot contain a slash (/)."
        })
    } else if name == "." || name == ".." {
        Some("Choose a name other than . or ..")
    } else if name.contains('\0') {
        Some(if folder {
            "Folder names cannot contain null characters."
        } else {
            "Filenames cannot contain null characters."
        })
    } else {
        None
    }
}

impl Workspace {
    pub(super) fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy || self.close_prompt || self.file_prompt.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else {
            self.show_notice("Open a folder to create a new file.", cx);
            return;
        };
        self.close_quick_open(window, cx);
        self.dismiss_tab_menu(window, cx);
        self.dismiss_file_menu(window, cx);
        self.sidebar = Sidebar::Files;
        self.show_file_prompt(root, PromptKind::NewFile, window, cx);
    }
    pub(super) fn file_action(
        &mut self,
        action: FileAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.tree.as_ref().and_then(|tree| tree.read(cx).selected());
        if matches!(action, FileAction::Paste) {
            self.paste_file(window, cx);
        } else if let Some(path) = path {
            self.file_action_at(action, path, window, cx);
        }
    }
    pub(super) fn file_action_at(
        &mut self,
        action: FileAction,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || self.close_prompt {
            return;
        }
        self.dismiss_file_menu(window, cx);
        match action {
            FileAction::Copy | FileAction::Cut => {
                Self::copy_file(path, matches!(action, FileAction::Cut), cx)
            }
            FileAction::Paste => self.paste_file(window, cx),
            FileAction::Duplicate => self.request_file_operation(
                Operation {
                    source: path,
                    kind: OperationKind::Duplicate,
                },
                window,
                cx,
            ),
            FileAction::Trash => self.request_file_operation(
                Operation {
                    source: path,
                    kind: OperationKind::Trash,
                },
                window,
                cx,
            ),
            FileAction::Rename => self.show_file_prompt(path, PromptKind::Rename, window, cx),
            FileAction::Move => {
                let selection = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Move Here".into()),
                });
                cx.spawn_in(window, async move |this, cx| match selection.await {
                    Ok(Ok(Some(paths))) => {
                        if let Some(directory) = paths.first() {
                            let destination = directory.join(path.file_name().unwrap_or_default());
                            this.update_in(cx, |this, window, cx| {
                                this.request_file_operation(
                                    Operation {
                                        source: path,
                                        kind: OperationKind::MoveTo(destination),
                                    },
                                    window,
                                    cx,
                                )
                            })
                            .ok();
                        }
                    }
                    Ok(Err(error)) => {
                        this.update(cx, |this, cx| {
                            this.show_notice(format!("Couldn't choose a folder: {error}"), cx)
                        })
                        .ok();
                    }
                    _ => {}
                })
                .detach();
            }
        }
    }
    pub(super) fn show_file_prompt(
        &mut self,
        path: PathBuf,
        kind: PromptKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = if kind == PromptKind::Rename {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        };
        let directory = self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.read(cx).is_directory(&path));
        let selection = rename_selection(&name, directory);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(
                if kind == PromptKind::NewFolder {
                    "Folder name"
                } else {
                    "Filename"
                },
                cx,
            );
            input.set_text(name, cx);
            input.select_range(selection, cx);
            input
        });
        let input_changes = cx.subscribe(&input, |this, _, TextInputEvent::Changed, cx| {
            if let Some(prompt) = &mut this.file_prompt {
                prompt.error = None;
            }
            cx.notify();
        });
        input.focus_handle(cx).focus(window);
        self.file_prompt = Some(FilePrompt {
            path,
            kind,
            error: None,
            input,
            _input_changes: input_changes,
        });
        cx.notify();
    }
    fn copy_file(path: PathBuf, cut: bool, cx: &mut App) {
        let item = ClipboardItem::new_string_with_metadata(
            path.to_string_lossy().into_owned(),
            format!(
                "plastext-file:{}:{}",
                std::process::id(),
                if cut { "cut" } else { "copy" }
            ),
        );
        cx.write_to_clipboard(item.clone());
        // GPUI has no file clipboard type. Add the native URL without replacing
        // its text/metadata, so Finder and other file managers can paste it too.
        #[cfg(not(test))]
        files::write_clipboard_url(&path);
        cx.set_global(FileClipboard { item, path, cut });
    }
    fn clipboard_file(cx: &App) -> Option<(PathBuf, bool)> {
        let current = cx.read_from_clipboard();
        if let Some(clipboard) = cx.try_global::<FileClipboard>()
            && current.as_ref() == Some(&clipboard.item)
        {
            return Some((clipboard.path.clone(), clipboard.cut));
        }
        #[cfg(not(test))]
        {
            files::read_clipboard_url().map(|path| (path, false))
        }
        #[cfg(test)]
        {
            None
        }
    }
    pub(super) fn can_paste_file(cx: &App) -> bool {
        Self::clipboard_file(cx).is_some()
    }
    fn paste_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = self
            .tree
            .as_ref()
            .map(|tree| tree.read(cx).selected_directory())
        else {
            return;
        };
        self.paste_file_into(directory, window, cx);
    }
    pub(super) fn paste_file_into(
        &mut self,
        directory: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((source, cut)) = Self::clipboard_file(cx) else {
            return;
        };
        let kind = if cut {
            OperationKind::MoveTo(directory.join(source.file_name().unwrap_or_default()))
        } else {
            OperationKind::CopyInto(directory)
        };
        self.request_file_operation(Operation { source, kind }, window, cx);
    }
    fn cancel_file_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy {
            return;
        }
        self.file_prompt = None;
        if let Some(tree) = &self.tree {
            tree.focus_handle(cx).focus(window);
        }
        cx.notify();
    }
    fn submit_file_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy || self.close_prompt {
            return;
        }
        let Some(prompt) = &self.file_prompt else {
            return;
        };
        let name = prompt.input.read(cx).text();
        if prompt.kind.error(name).is_some() || prompt.error.is_some() {
            return;
        }
        if prompt.kind != PromptKind::Rename {
            let kind = if prompt.kind == PromptKind::NewFolder {
                OperationKind::CreateFolder
            } else {
                OperationKind::CreateFile
            };
            self.start_file_operation(
                Operation {
                    source: prompt.path.join(name),
                    kind,
                },
                window,
                cx,
            );
            return;
        }
        let source = prompt.path.clone();
        let destination = source.with_file_name(name);
        self.cancel_file_prompt(window, cx);
        self.request_file_operation(
            Operation {
                source,
                kind: OperationKind::MoveTo(destination),
            },
            window,
            cx,
        );
    }
    pub(super) fn render_file_prompt(
        &self,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.file_prompt.as_ref()?;
        let error = prompt
            .kind
            .error(prompt.input.read(cx).text())
            .map(SharedString::from)
            .or_else(|| prompt.error.clone());
        let enabled = error.is_none() && !self.file_busy;
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(gpui::rgba(0x00000066))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(80.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .capture_key_down(cx.listener(|this, _, window, cx| {
                    if this.file_busy {
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => this.cancel_file_prompt(window, cx),
                        "enter" => this.submit_file_prompt(window, cx),
                        _ => return,
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(420.))
                        .p_4()
                        .rounded_md()
                        .shadow_lg()
                        .bg(theme.surface)
                        .border_1()
                        .border_color(theme.border)
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(prompt.kind.title())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(prompt.input.clone())
                                .child(
                                    div()
                                        .min_h(px(18.))
                                        .text_xs()
                                        .text_color(theme.text)
                                        .child(error.unwrap_or_default()),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_3()
                                .child(
                                    div()
                                        .id("cancel-rename")
                                        .px_3()
                                        .py_1()
                                        .cursor_pointer()
                                        .child("Cancel")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel_file_prompt(window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .id("confirm-rename")
                                        .debug_selector(|| "confirm-rename".into())
                                        .px_3()
                                        .py_1()
                                        .rounded_sm()
                                        .bg(theme.border)
                                        .child(prompt.kind.confirm())
                                        .when(!enabled, |button| button.opacity(0.5))
                                        .when(enabled, |button| {
                                            button.cursor_pointer().on_click(cx.listener(
                                                |this, _, window, cx| {
                                                    this.submit_file_prompt(window, cx)
                                                },
                                            ))
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
    pub(super) fn request_file_operation(
        &mut self,
        operation: Operation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || self.close_prompt {
            return;
        }
        if matches!(&operation.kind, OperationKind::MoveTo(destination) if *destination == operation.source)
        {
            return;
        }
        if operation.changes_source() {
            self.confirm_close(editing::CloseTarget::FileOperation(operation), window, cx);
        } else {
            self.start_file_operation(operation, window, cx);
        }
    }
    pub(super) fn start_file_operation(
        &mut self,
        operation: Operation,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let documents: Vec<_> = EditingDocument::all(cx)
            .into_iter()
            .filter(|doc| {
                operation.changes_source() && doc.read(cx).path().starts_with(&operation.source)
            })
            .collect();
        if documents.iter().any(|doc| {
            let doc = doc.read(cx);
            doc.has_pending_input()
                || doc.is_saving()
                || doc.is_preparing()
                || doc.file_operation_pending()
        }) {
            self.show_notice("Finish the pending edit or file operation first.", cx);
            return;
        }
        for doc in &documents {
            doc.update(cx, |doc, cx| doc.set_file_operation_pending(true, cx));
        }
        self.file_busy = true;
        self.notice = Some(
            match operation.kind {
                OperationKind::Duplicate | OperationKind::CopyInto(_) => "Copying…",
                OperationKind::MoveTo(_) => "Moving…",
                OperationKind::Trash => "Moving to Bin…",
                OperationKind::CreateFile => "Creating file…",
                OperationKind::CreateFolder => "Creating folder…",
            }
            .into(),
        );
        let clipboard = cx.read_from_clipboard();
        let work = cx.background_executor().spawn({
            let operation = operation.clone();
            async move { operation.run() }
        });
        let origin = cx.entity().entity_id();
        // App-scoped completion unlocks documents even if the originating window closes.
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.file_busy = false;
                cx.notify();
            })
            .ok();
            match result {
                Ok(destination) => {
                    let _ = cx.update(|cx| {
                        let windows = cx.windows();
                        for window in windows {
                            if let Some(workspace) = window.downcast::<Workspace>() {
                                workspace
                                    .update(cx, |workspace, window, cx| {
                                        workspace.finish_file_operation(
                                            &operation,
                                            destination.as_deref(),
                                            window,
                                            cx,
                                        );
                                        if cx.entity().entity_id() == origin
                                            && operation.is_creation()
                                        {
                                            workspace.cancel_file_prompt(window, cx);
                                            if matches!(operation.kind, OperationKind::CreateFile) {
                                                workspace.open(
                                                    operation.source.clone(),
                                                    Some(Target::Source),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }
                                    })
                                    .ok();
                            }
                        }
                        if matches!(operation.kind, OperationKind::MoveTo(_))
                            && cx.read_from_clipboard() == clipboard
                            && let Some(stored) = cx.try_global::<FileClipboard>()
                            && stored.cut
                            && stored.path == operation.source
                            && let Some(destination) = &destination
                        {
                            Self::copy_file(destination.clone(), false, cx);
                        }
                    });
                }
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if operation.is_creation()
                            && let Some(prompt) = &mut this.file_prompt
                        {
                            prompt.error =
                                Some(if error.kind() == std::io::ErrorKind::AlreadyExists {
                                    "An item with that name already exists.".into()
                                } else {
                                    format!("Couldn't create this item: {error}").into()
                                });
                            this.notice = None;
                            cx.notify();
                        } else {
                            this.show_notice(
                                format!("Couldn't change {}: {error}", operation.source.display()),
                                cx,
                            );
                        }
                    })
                    .ok();
                }
            }
            for doc in documents {
                doc.update(cx, |doc, cx| doc.set_file_operation_pending(false, cx))
                    .ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn finish_file_operation(
        &mut self,
        operation: &Operation,
        destination: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused_tree = self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.focus_handle(cx).is_focused(window));
        let relevant_tree = self.root.as_ref().is_some_and(|root| {
            operation.source.starts_with(root)
                || destination.is_some_and(|path| path.starts_with(root))
        });
        let relevant_tabs = self.panes.values().any(|pane| {
            pane.tabs
                .iter()
                .any(|tab| tab.path.starts_with(&operation.source))
        });
        if !relevant_tree && !relevant_tabs {
            return;
        }
        if operation.changes_source() {
            let active_pane = self.active_pane;
            for pane_id in self.layout.panes() {
                let pane = self.panes.get_mut(&pane_id).unwrap();
                pane.opening.cancel_under(&operation.source);
                if !pane
                    .tabs
                    .iter()
                    .any(|tab| tab.path.starts_with(&operation.source))
                {
                    continue;
                }
                let active = pane.active_tab().map(|tab| tab.path.clone());
                let mut reopen = Vec::new();
                let mut kept = Vec::new();
                for tab in pane.tabs.drain(..) {
                    if let Ok(relative) = tab.path.strip_prefix(&operation.source) {
                        tab.view.set_visible(false, cx);
                        if let Some(destination) = destination {
                            reopen.push((
                                relocated_path(destination, relative),
                                tab.view.position(cx),
                                active.as_ref() == Some(&tab.path),
                            ));
                        }
                    } else {
                        kept.push(tab);
                    }
                }
                pane.tabs = kept;
                pane.active = active
                    .as_ref()
                    .and_then(|path| pane.tabs.iter().position(|tab| &tab.path == path))
                    .or_else(|| (!pane.tabs.is_empty()).then_some(0));
                reopen.sort_by_key(|(_, _, active)| *active);
                self.active_pane = pane_id;
                let restores_active = reopen.iter().any(|(_, _, active)| *active);
                for (path, offset, _) in reopen {
                    self.open_with_focus(
                        path,
                        Some(Target::Offset(offset)),
                        OpenKind::Keep,
                        false,
                        window,
                        cx,
                    );
                }
                if !restores_active {
                    self.panes.get_mut(&pane_id).unwrap().opening.select();
                }
            }
            self.active_pane = active_pane;
            {
                self.recent
                    .retain_mut(|path| match path.strip_prefix(&operation.source) {
                        Ok(relative) => {
                            if let Some(destination) = destination {
                                *path = relocated_path(destination, relative);
                                true
                            } else {
                                false
                            }
                        }
                        Err(_) => true,
                    });
            }
            for places in [&mut self.back, &mut self.forward] {
                places.retain_mut(|place| match place.path.strip_prefix(&operation.source) {
                    Ok(relative) => {
                        if let Some(destination) = destination {
                            place.path = relocated_path(destination, relative);
                            true
                        } else {
                            false
                        }
                    }
                    Err(_) => true,
                });
            }
        }
        if let Some(tree) = &self.tree {
            tree.update(cx, |tree, cx| {
                tree.refresh(
                    &operation.source,
                    destination,
                    operation.changes_source(),
                    cx,
                )
            });
            if focused_tree {
                tree.focus_handle(cx).focus(window);
            }
        }
        self.notice = None;
        cx.notify();
    }
}

fn relocated_path(destination: &Path, relative: &Path) -> PathBuf {
    if relative.as_os_str().is_empty() {
        destination.to_owned()
    } else {
        destination.join(relative)
    }
}

#[cfg(test)]
#[path = "file_actions_tests.rs"]
mod tests;
