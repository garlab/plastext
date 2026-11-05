use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle, point};

fn open(cx: &mut TestAppContext, root: &Path) -> WindowHandle<Workspace> {
    cx.update(crate::init);
    let window = cx
        .add_window(|window, cx| Workspace::new(Some(root.to_owned()), Arc::default(), window, cx));
    cx.run_until_parked();
    window
}
fn click_row(cx: &mut TestAppContext, window: WindowHandle<Workspace>, row: usize) {
    VisualTestContext::from_window(window.into(), cx).simulate_click(
        point(px(80.), px(TAB_BAR_HEIGHT + 4. + 24. * row as f32 + 12.)),
        Default::default(),
    );
    cx.run_until_parked();
}
fn select(cx: &mut TestAppContext, window: WindowHandle<Workspace>, path: &Path) {
    window
        .update(cx, |workspace, window, cx| {
            let tree = workspace.tree.as_ref().unwrap();
            tree.update(cx, |tree, cx| tree.select(Some(path.to_owned()), cx));
            tree.focus_handle(cx).focus(window);
        })
        .unwrap();
    cx.run_until_parked();
}
fn close(cx: &mut TestAppContext, window: WindowHandle<Workspace>) {
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn single_click_keeps_tree_focus_and_delete_shortcut_targets_selected_file(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let window = open(cx, dir.path());
    click_row(cx, window, 0);
    window
        .update(cx, |workspace, window, cx| {
            assert!(
                workspace
                    .tree
                    .as_ref()
                    .unwrap()
                    .focus_handle(cx)
                    .is_focused(window)
            );
            let tab = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap();
            assert!(tab.preview);
            assert_eq!(tab.path, path);
            assert!(!tab.view.is_dirty(cx));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "x");
    window
        .update(cx, |workspace, _, cx| {
            assert!(
                !workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .unwrap()
                    .view
                    .is_dirty(cx)
            )
        })
        .unwrap();
    // Enter explicitly enters the editor and pins the preview tab.
    cx.simulate_keystrokes(window.into(), "enter x");
    window
        .update(cx, |workspace, window, cx| {
            let tab = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap();
            assert!(tab.view.focus_handle(cx).is_focused(window));
            assert!(!tab.preview);
            assert!(tab.view.is_dirty(cx));
        })
        .unwrap();
    click_row(cx, window, 0); // Existing tabs must also leave focus in the tree.
    cx.simulate_keystrokes(window.into(), "cmd-backspace");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(path.exists());
    window
        .update(cx, |workspace, window, cx| {
            assert!(
                workspace
                    .tree
                    .as_ref()
                    .unwrap()
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert!(
                workspace.panes[&workspace.active_pane]
                    .active_tab()
                    .unwrap()
                    .view
                    .is_dirty(cx)
            );
        })
        .unwrap();
    close(cx, window);
}

#[gpui::test]
fn copy_cut_paste_and_duplicate_refresh_tree_without_overwriting(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    let folder = dir.path().join("folder");
    std::fs::write(&path, "content").unwrap();
    std::fs::create_dir(&folder).unwrap();
    let window = open(cx, dir.path());
    select(cx, window, &path);
    cx.simulate_keystrokes(window.into(), "cmd-c cmd-v");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes copy.txt")).unwrap(),
        "content"
    );
    select(cx, window, &path);
    cx.simulate_keystrokes(window.into(), "cmd-x");
    assert!(path.exists(), "Cut is deferred until Paste");
    select(cx, window, &folder);
    cx.simulate_keystrokes(window.into(), "cmd-v");
    let moved = folder.join("notes.txt");
    assert!(!path.exists());
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "content");
    window
        .update(cx, |workspace, window, cx| {
            workspace.file_action_at(FileAction::Duplicate, moved.clone(), window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(folder.join("notes copy.txt")).unwrap(),
        "content"
    );
    // Copying ordinary text invalidates a previous file Cut/Copy.
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("plain text".into())));
    cx.simulate_keystrokes(window.into(), "cmd-v");
    assert!(!folder.join("notes copy 2.txt").exists());
    close(cx, window);
}

#[gpui::test]
fn rename_dialog_cancels_safely_and_renames_open_files_after_explicit_save(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("code.rs");
    let destination = dir.path().join("new.rs");
    std::fs::write(&source, "before").unwrap();
    let window = open(cx, dir.path());
    click_row(cx, window, 0);
    cx.simulate_keystrokes(window.into(), "f2 escape");
    assert!(source.exists());
    cx.simulate_keystrokes(window.into(), "enter x");
    select(cx, window, &source);
    cx.simulate_keystrokes(window.into(), "f2");
    window
        .update(cx, |workspace, _, cx| {
            workspace
                .file_prompt
                .as_ref()
                .unwrap()
                .input
                .update(cx, |input, cx| input.set_text("new.rs", cx));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    cx.run_until_parked();
    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "xbefore");
    window
        .update(cx, |workspace, window, cx| {
            assert!(!workspace.file_busy);
            let tab = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap();
            assert_eq!(tab.path, destination);
            assert_eq!(
                tab.view.editing_document(cx).unwrap().read(cx).path(),
                destination
            );
            assert!(!tab.view.is_dirty(cx));
            assert!(
                workspace
                    .tree
                    .as_ref()
                    .unwrap()
                    .focus_handle(cx)
                    .is_focused(window)
            );
        })
        .unwrap();
    // Saving afterwards must use the new path, never recreate the old file.
    cx.simulate_keystrokes(window.into(), "enter y cmd-s");
    assert!(!source.exists());
    assert!(std::fs::read_to_string(&destination).unwrap().contains('y'));
    close(cx, window);
}

#[gpui::test]
fn failed_move_keeps_unsaved_buffer_and_all_tabs(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("a.txt");
    let destination = dir.path().join("b.txt");
    std::fs::write(&source, "original").unwrap();
    std::fs::write(&destination, "must survive").unwrap();
    let window = open(cx, dir.path());
    click_row(cx, window, 0);
    cx.simulate_keystrokes(window.into(), "enter x");
    window
        .update(cx, |workspace, window, cx| {
            workspace.request_file_operation(
                Operation {
                    source: source.clone(),
                    kind: OperationKind::MoveTo(destination.clone()),
                },
                window,
                cx,
            )
        })
        .unwrap();
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "must survive"
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "original");
    window
        .update(cx, |workspace, _, cx| {
            let tab = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap();
            assert_eq!(tab.path, source);
            assert!(tab.view.is_dirty(cx));
            assert!(
                !tab.view
                    .editing_document(cx)
                    .unwrap()
                    .read(cx)
                    .file_operation_pending()
            );
            assert!(workspace.notice.is_some());
        })
        .unwrap();
    close(cx, window);
}

#[gpui::test]
fn moving_folder_reopens_each_split_at_the_new_path(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("folder");
    let destination = dir.path().join("renamed");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.txt"), "a").unwrap();
    std::fs::write(source.join("b.txt"), "b").unwrap();
    let window = open(cx, dir.path());
    window
        .update(cx, |workspace, window, cx| {
            workspace.open_file(source.join("a.txt"), None, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, cx| {
            workspace.split(Axis::Horizontal, window, cx);
            workspace.open_file(source.join("b.txt"), None, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    select(cx, window, &source);
    window
        .update(cx, |workspace, window, cx| {
            workspace.request_file_operation(
                Operation {
                    source: source.clone(),
                    kind: OperationKind::MoveTo(destination.clone()),
                },
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!source.exists());
    window
        .update(cx, |workspace, window, cx| {
            assert_eq!(workspace.panes.len(), 2);
            let active: Vec<_> = workspace
                .layout
                .panes()
                .into_iter()
                .map(|id| {
                    let pane = &workspace.panes[&id];
                    assert!(
                        pane.tabs
                            .iter()
                            .all(|tab| tab.path.starts_with(&destination))
                    );
                    pane.active_tab().unwrap().path.clone()
                })
                .collect();
            assert_eq!(
                active,
                [destination.join("a.txt"), destination.join("b.txt")]
            );
            assert!(
                workspace
                    .tree
                    .as_ref()
                    .unwrap()
                    .focus_handle(cx)
                    .is_focused(window)
            );
        })
        .unwrap();
    close(cx, window);
}
