//! Native menus dispatch the same actions as keyboard shortcuts, preserving
//! focus, read-only restrictions and Save / Discard / Cancel protections.
use super::{AppSettings, Quit, WindowRequest, create_window, open_window};
use gpui::{
    App, KeyBinding, Menu, MenuItem, OsAction, PathPromptOptions, SystemMenuType, WindowHandle,
    actions,
};
use plastext_cli::Location;
use plastext_editor::actions as edit;
use plastext_ui::{Workspace, actions as ui};
use std::path::PathBuf;

actions!(
    application,
    [
        NewWindow,
        OpenFile,
        OpenFolder,
        About,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        ZoomWindow,
        FullScreen,
        Documentation,
        ReportIssue
    ]
);

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-o", OpenFile, None),
        KeyBinding::new("cmd-alt-o", OpenFolder, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", FullScreen, None),
    ]);
    cx.on_action(|_: &NewWindow, cx| {
        if cx.has_global::<AppSettings>() {
            create_window(
                WindowRequest {
                    root: None,
                    files: Vec::new(),
                },
                None,
                cx,
            );
        }
    });
    cx.on_action(|_: &OpenFile, cx| prompt_paths(false, cx));
    cx.on_action(|_: &OpenFolder, cx| prompt_paths(true, cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, _| window.minimize_window())
                .ok();
        }
    });
    cx.on_action(|_: &ZoomWindow, cx| {
        if let Some(window) = cx.active_window() {
            window.update(cx, |_, window, _| window.zoom_window()).ok();
        }
    });
    cx.on_action(|_: &FullScreen, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, _| window.toggle_fullscreen())
                .ok();
        }
    });
    cx.on_action(|_: &About, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, cx| {
                    drop(window.prompt(
                        gpui::PromptLevel::Info,
                        "Plastext",
                        Some(concat!(
                            "Version ",
                            env!("CARGO_PKG_VERSION"),
                            "\nA native viewer and light editor for code, Markdown and assets."
                        )),
                        &["OK"],
                        cx,
                    ));
                })
                .ok();
        }
    });
    cx.on_action(|_: &Documentation, cx| cx.open_url("https://github.com/garlab/plastext#readme"));
    cx.on_action(|_: &ReportIssue, cx| cx.open_url("https://github.com/garlab/plastext/issues"));
    cx.set_menus(menus());
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "plastext".into(),
            items: vec![
                MenuItem::action("About Plastext", About),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Plastext", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Plastext", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New File…", ui::NewFile),
                MenuItem::action("New Window", NewWindow),
                MenuItem::separator(),
                MenuItem::action("Open File…", OpenFile),
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::separator(),
                MenuItem::action("Save", edit::Save),
                MenuItem::separator(),
                MenuItem::action("Close Tab", ui::CloseTab),
                MenuItem::action("Close Window", ui::CloseWindow),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", edit::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", edit::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", edit::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", edit::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", edit::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", edit::SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Find…", edit::Find),
                MenuItem::action("Find Next", edit::FindNext),
                MenuItem::action("Find Previous", edit::FindPrevious),
                MenuItem::action("Find in Files…", ui::SearchProject),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", ui::OpenCommandPalette),
                MenuItem::action("Show Files", ui::ShowFiles),
                MenuItem::action("Toggle Outline", ui::ToggleOutline),
                MenuItem::action("Switch Source / Preview", ui::TogglePreview),
                MenuItem::separator(),
                MenuItem::action("Zoom In", ui::ZoomIn),
                MenuItem::action("Zoom Out", ui::ZoomOut),
                MenuItem::action("Actual Size", ui::ResetZoom),
                MenuItem::separator(),
                MenuItem::action("Split Right", ui::SplitRight),
                MenuItem::action("Split Down", ui::SplitDown),
                MenuItem::separator(),
                MenuItem::action("File Icon Theme…", ui::FileIconTheme),
                MenuItem::action("Import Icon Theme…", ui::ImportIconTheme),
            ],
        },
        Menu {
            name: "Go".into(),
            items: vec![
                MenuItem::action("Back", ui::GoBack),
                MenuItem::action("Forward", ui::GoForward),
                MenuItem::separator(),
                MenuItem::action("Go to File…", ui::OpenQuickOpen),
                MenuItem::action("Go to Line…", ui::GoToLine),
                MenuItem::action("Go to Symbol…", ui::GoToSymbol),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", ZoomWindow),
                MenuItem::action("Toggle Full Screen", FullScreen),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Plastext Documentation", Documentation),
                MenuItem::action("Report an Issue…", ReportIssue),
            ],
        },
    ]
}

fn prompt_paths(folders: bool, cx: &mut App) {
    let target = cx
        .active_window()
        .and_then(|window| window.downcast::<Workspace>());
    let selection = cx.prompt_for_paths(PathPromptOptions {
        files: !folders,
        directories: folders,
        multiple: !folders,
        prompt: Some(if folders { "Open Folder" } else { "Open File" }.into()),
    });
    cx.spawn(async move |cx| match selection.await {
        Ok(Ok(Some(paths))) => {
            cx.update(|cx| {
                if folders {
                    for root in paths {
                        open_window(
                            WindowRequest {
                                root: Some(root),
                                files: Vec::new(),
                            },
                            None,
                            cx,
                        );
                    }
                } else {
                    open_files(paths, target, cx);
                }
            })
            .ok();
        }
        Ok(Err(error)) => {
            if let Some(target) = target {
                target
                    .update(cx, |workspace, _, cx| {
                        workspace.show_notice(format!("Couldn't open the file picker: {error}"), cx)
                    })
                    .ok();
            }
        }
        _ => {}
    })
    .detach();
}

fn open_files(paths: Vec<PathBuf>, target: Option<WindowHandle<Workspace>>, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    if let Some(target) = target
        && target
            .update(cx, |workspace, window, cx| {
                for path in &paths {
                    workspace.open_file(path.clone(), None, window, cx);
                }
                window.activate_window();
            })
            .is_ok()
    {
        return;
    }
    open_window(
        WindowRequest {
            root: paths[0].parent().map(ToOwned::to_owned),
            files: paths
                .into_iter()
                .map(|path| Location {
                    path,
                    line: None,
                    column: None,
                })
                .collect(),
        },
        None,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Action, TestAppContext};
    use std::sync::Arc;

    fn action(menu: &str, label: &str) -> Box<dyn Action> {
        menus()
            .into_iter()
            .find(|m| m.name == menu)
            .unwrap()
            .items
            .into_iter()
            .find_map(|item| match item {
                MenuItem::Action { name, action, .. } if name == label => Some(action),
                _ => None,
            })
            .unwrap()
    }

    #[gpui::test]
    fn opening_files_keeps_the_project_and_new_window_does_not_reuse_it(cx: &mut TestAppContext) {
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let paths = [
            project.path().join("first.txt"),
            other.path().join("second.txt"),
        ];
        for path in &paths {
            std::fs::write(path, "content\n").unwrap();
        }
        cx.update(|cx| {
            plastext_ui::init(cx);
            init(cx);
            cx.set_global(AppSettings(Arc::default()));
        });
        let window = cx.add_window(|window, cx| {
            Workspace::new(Some(project.path().to_owned()), Arc::default(), window, cx)
        });
        cx.update(|cx| open_files(paths.to_vec(), Some(window), cx));
        cx.run_until_parked();
        window
            .update(cx, |workspace, _, cx| {
                assert_eq!(workspace.root(), Some(project.path()));
                let documents = plastext_editor::EditingDocument::all(cx);
                for path in &paths {
                    assert!(documents.iter().any(|doc| doc.read(cx).path() == path));
                }
                assert_eq!(cx.windows().len(), 1);
            })
            .unwrap();
        // Dispatch the menu's actual action. Repeated New Window commands must
        // each create a window, even when an empty workspace already exists.
        for expected in [2, 3] {
            cx.update(|cx| cx.dispatch_action(action("File", "New Window").as_ref()));
            cx.run_until_parked();
            assert_eq!(cx.update(|cx| cx.windows().len()), expected);
        }
        // Opening an existing project uses its window without replacing tabs.
        cx.update(|cx| {
            open_window(
                WindowRequest {
                    root: Some(project.path().to_owned()),
                    files: Vec::new(),
                },
                None,
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), 3);
        assert_eq!(
            cx.update(|cx| plastext_editor::EditingDocument::all(cx).len()),
            2
        );
    }
}
