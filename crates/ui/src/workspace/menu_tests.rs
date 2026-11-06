use super::*;
use gpui::{Action, ClipboardItem, TestAppContext, WindowHandle};
use plastext_buffer::ByteSource;
use plastext_editor::actions as edit;

fn open(cx: &mut TestAppContext, path: &Path) -> WindowHandle<Workspace> {
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace = Workspace::new(
            path.parent().map(ToOwned::to_owned),
            Arc::default(),
            window,
            cx,
        );
        workspace.open_file(path.to_owned(), None, window, cx);
        workspace
    });
    cx.run_until_parked();
    window
}

fn dispatch(cx: &mut TestAppContext, window: WindowHandle<Workspace>, action: impl Action) {
    window
        .update(cx, |_, window, cx| {
            assert!(window.is_action_available(&action, cx), "{}", action.name());
            window.dispatch_action(Box::new(action), cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn text(cx: &mut TestAppContext, window: WindowHandle<Workspace>) -> String {
    window
        .update(cx, |workspace, _, cx| {
            let doc = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap()
                .view
                .editing_document(cx)
                .unwrap();
            let version = doc.read(cx).version().unwrap();
            let snapshot = version.text();
            snapshot.load(0..snapshot.len()).unwrap();
            String::from_utf8(snapshot.bytes(0..snapshot.len()).unwrap().to_vec()).unwrap()
        })
        .unwrap()
}

fn clipboard(cx: &TestAppContext, text: &str) {
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned())));
}

fn focus_tree(cx: &mut TestAppContext, window: WindowHandle<Workspace>, path: &Path) {
    window
        .update(cx, |workspace, window, cx| {
            let tree = workspace.tree.as_ref().unwrap();
            tree.update(cx, |tree, cx| tree.select(Some(path.to_owned()), cx));
            tree.focus_handle(cx).focus(window);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn welcome_buttons_and_recent_folders_dispatch_shared_menu_actions(cx: &mut TestAppContext) {
    use crate::actions::{ClearRecentFolders, OpenFile, OpenFolder, OpenRecentFolder};
    use std::cell::RefCell;
    let calls = Rc::new(RefCell::new(Vec::new()));
    let folder = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        crate::init(cx);
        cx.set_global(welcome::RecentFolders(vec![folder.path().to_owned()]));
        let opened = calls.clone();
        cx.on_action(move |_: &OpenFolder, _| opened.borrow_mut().push("folder"));
        let opened = calls.clone();
        cx.on_action(move |_: &OpenFile, _| opened.borrow_mut().push("file"));
        let opened = calls.clone();
        let path = folder.path().to_owned();
        cx.on_action(move |action: &OpenRecentFolder, _| {
            assert_eq!(action.path, path);
            opened.borrow_mut().push("recent");
        });
        let opened = calls.clone();
        cx.on_action(move |_: &ClearRecentFolders, cx| {
            opened.borrow_mut().push("clear");
            cx.set_global(welcome::RecentFolders::default());
        });
    });
    let window = cx.add_window(|window, cx| Workspace::new(None, Arc::default(), window, cx));
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let content = visual.debug_bounds("welcome-content").unwrap();
    let viewport = visual.debug_bounds("welcome").unwrap();
    assert_eq!(content.center().x, viewport.center().x);
    assert!((content.center().y - viewport.center().y).abs() <= px(1.));
    assert!(content.size.width <= px(520.));
    for selector in [
        "welcome-open-folder",
        "welcome-open-file",
        "welcome-recent-0",
        "welcome-clear-recent",
    ] {
        let bounds = visual.debug_bounds(selector).unwrap();
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        visual.simulate_click(bounds.center(), Default::default());
    }
    cx.run_until_parked();
    assert_eq!(*calls.borrow(), ["folder", "file", "recent", "clear"]);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    // GPUI retains old debug selectors between frames; use the newly laid-out
    // content bounds to verify that clearing history removed the recent list.
    assert!(visual.debug_bounds("welcome-content").unwrap().size.height < content.size.height);
    let content = visual.debug_bounds("welcome-content").unwrap();
    assert!((content.center().y - viewport.center().y).abs() <= px(1.));
}

#[gpui::test]
fn welcome_stays_usable_in_small_windows_with_a_full_recent_list(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::init(cx);
        cx.set_global(welcome::RecentFolders(
            (0..10)
                .map(|i| {
                    std::env::temp_dir().join(format!("Café… 中文 🦀 — long project name {i}"))
                })
                .collect(),
        ));
    });
    let window = cx.add_window(|window, cx| Workspace::new(None, Arc::default(), window, cx));
    let visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(480.), px(420.)));
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let viewport = visual.debug_bounds("welcome").unwrap();
    let content = visual.debug_bounds("welcome-content").unwrap();
    let recent = visual.debug_bounds("welcome-recent-list").unwrap();
    assert_eq!(content.center().x, viewport.center().x);
    assert!(
        content.origin.y >= viewport.origin.y,
        "content must start above the fold, not scroll off the top"
    );
    assert!(content.size.width <= viewport.size.width - px(48.));
    assert!(recent.size.height <= px(216.));
    for selector in ["welcome-open-folder", "welcome-open-file"] {
        let button = visual.debug_bounds(selector).unwrap();
        assert!(
            viewport.contains(&button.center()),
            "{selector} must remain visible"
        );
    }
}

#[gpui::test]
fn close_tab_is_available_only_while_a_tab_is_open(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    std::fs::write(&path, "Original\n").unwrap();
    let window = open(cx, &path);
    dispatch(cx, window, SplitRight);
    dispatch(cx, window, CloseTab);
    // Closing one split still leaves a tab that can be closed.
    dispatch(cx, window, CloseTab);
    window
        .update(cx, |workspace, window, cx| {
            assert!(
                workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .is_none()
            );
            assert!(!window.is_action_available(&CloseTab, cx));
            assert!(window.is_action_available(&CloseWindow, cx));
            workspace.open_file(path.clone(), None, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            assert!(window.is_action_available(&CloseTab, cx));
        })
        .unwrap();
}

#[gpui::test]
fn edit_menu_targets_the_editor_search_field_and_file_tree(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    std::fs::write(&path, "Original\n").unwrap();
    let window = open(cx, &path);
    dispatch(cx, window, edit::SelectAll);
    dispatch(cx, window, edit::Cut);
    assert_eq!(text(cx, window), "");
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
        "Original\n"
    );
    dispatch(cx, window, edit::Undo);
    assert_eq!(text(cx, window), "Original\n");
    dispatch(cx, window, edit::Redo);
    assert_eq!(text(cx, window), "");
    clipboard(cx, "Changed\n");
    dispatch(cx, window, edit::Paste);
    assert_eq!(text(cx, window), "Changed\n");

    dispatch(cx, window, edit::Find);
    clipboard(cx, "query");
    dispatch(cx, window, edit::Paste);
    dispatch(cx, window, edit::SelectAll);
    dispatch(cx, window, edit::Copy);
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
        "query"
    );
    assert_eq!(
        text(cx, window),
        "Changed\n",
        "search-field commands must not edit the document"
    );
    window
        .update(cx, |_, window, cx| {
            assert!(!window.is_action_available(&edit::Undo, cx))
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");

    focus_tree(cx, window, &path);
    dispatch(cx, window, edit::Save);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Changed\n");
    dispatch(cx, window, edit::Copy);
    dispatch(cx, window, edit::Paste);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note copy.txt")).unwrap(),
        "Changed\n"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Changed\n");
}

#[gpui::test]
fn preview_copy_toggle_and_window_close_keep_unsaved_changes_safe(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    std::fs::write(&path, "# Original\n").unwrap();
    let window = open(cx, &path);
    window
        .update(cx, |_, window, cx| {
            assert!(!window.is_action_available(&edit::Paste, cx));
            assert!(!window.is_action_available(&edit::Cut, cx));
            assert!(!window.is_action_available(&edit::Undo, cx));
        })
        .unwrap();
    dispatch(cx, window, edit::SelectAll);
    dispatch(cx, window, edit::Copy);
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap()),
        "Original"
    );
    dispatch(cx, window, crate::actions::TogglePreview);
    dispatch(cx, window, edit::SelectAll);
    clipboard(cx, "# Changed\n");
    dispatch(cx, window, edit::Paste);
    dispatch(cx, window, crate::actions::TogglePreview);
    dispatch(cx, window, CloseWindow);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text(cx, window), "# Changed\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Original\n");
    dispatch(cx, window, edit::Save);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Changed\n");
    dispatch(cx, window, CloseWindow);
    assert!(cx.update(|cx| cx.windows().is_empty()));
}

#[gpui::test]
fn readonly_views_disable_edit_menu_mutations_and_csv_can_toggle(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("table.csv");
    std::fs::write(&path, "name,value\nalpha,1\n").unwrap();
    let window = open(cx, &path);
    for _ in 0..2 {
        window
            .update(cx, |_, window, cx| {
                for action in [
                    &edit::Cut as &dyn Action,
                    &edit::Paste,
                    &edit::Undo,
                    &edit::Redo,
                    &edit::Save,
                    &edit::ShowCharacterPalette,
                ] {
                    assert!(!window.is_action_available(action, cx), "{}", action.name());
                }
            })
            .unwrap();
        dispatch(cx, window, crate::actions::TogglePreview);
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "name,value\nalpha,1\n"
    );
}

#[gpui::test]
fn markdown_source_button_keeps_editing_and_character_palette_follows_focus(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    std::fs::write(&path, "# Original\n").unwrap();
    let window = open(cx, &path);
    for _ in 0..3 {
        focus_tree(cx, window, &path);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        let button = visual.debug_bounds("md-toggle-source").unwrap();
        visual.simulate_click(button.center(), Default::default());
        cx.run_until_parked();
        window.update(cx, |workspace, window, cx| {
            let tab = workspace.panes[&workspace.active_pane].active_tab().unwrap();
            assert!(matches!(tab.view.status(cx), crate::tab_view::TabStatus::Text(status) if status.editing));
            assert!(window.is_action_available(&edit::Paste, cx));
            assert!(window.is_action_available(&edit::ShowCharacterPalette, cx));
        }).unwrap();
        dispatch(cx, window, edit::ToggleWordWrap);
        window
            .update(cx, |_, window, cx| {
                assert!(plastext_editor::focused_word_wrap(window, cx))
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "alt-z");
        window
            .update(cx, |_, window, cx| {
                assert!(!plastext_editor::focused_word_wrap(window, cx))
            })
            .unwrap();
        dispatch(cx, window, edit::SelectAll);
        clipboard(cx, "# Changed 😀\n");
        dispatch(cx, window, edit::Paste);
        assert_eq!(text(cx, window), "# Changed 😀\n");
        cx.simulate_keystrokes(window.into(), "escape");
        dispatch(cx, window, crate::actions::TogglePreview);
        window
            .update(cx, |_, window, cx| {
                assert!(!window.is_action_available(&edit::ShowCharacterPalette, cx));
            })
            .unwrap();
    }
    dispatch(cx, window, edit::Find);
    window
        .update(cx, |_, window, cx| {
            assert!(window.is_action_available(&edit::ShowCharacterPalette, cx));
        })
        .unwrap();
}

#[cfg(unix)]
#[gpui::test]
fn readonly_markdown_source_never_enables_typing_or_emojis(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readonly.md");
    std::fs::write(&path, "# Read-only\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let window = open(cx, &path);
    for _ in 0..2 {
        dispatch(cx, window, crate::actions::TogglePreview);
        window.update(cx, |workspace, window, cx| {
            let tab = workspace.panes[&workspace.active_pane].active_tab().unwrap();
            assert!(matches!(tab.view.status(cx), crate::tab_view::TabStatus::Text(status) if !status.editing));
            assert!(!window.is_action_available(&edit::Paste, cx));
            assert!(!window.is_action_available(&edit::ShowCharacterPalette, cx));
            assert!(window.is_action_available(&edit::ToggleWordWrap, cx));
        }).unwrap();
        dispatch(cx, window, crate::actions::TogglePreview);
    }
}
