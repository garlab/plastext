use super::*;
use gpui::{ClipboardItem, TestAppContext, VisualTestContext, WindowHandle};
use plastext_buffer::ByteSource;
use plastext_editor::EditingDocument;

fn open(cx: &mut TestAppContext, root: Option<PathBuf>) -> WindowHandle<Workspace> {
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| Workspace::new(root, Arc::default(), window, cx));
    window
        .update(cx, |workspace, window, _| {
            workspace.focus_handle.focus(window)
        })
        .unwrap();
    cx.run_until_parked();
    window
}
fn click(cx: &mut TestAppContext, window: WindowHandle<Workspace>, id: &'static str) {
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let bounds = visual.debug_bounds(id).unwrap();
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
}
fn paste(cx: &mut TestAppContext, window: WindowHandle<Workspace>, text: &str) {
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text.into())));
    cx.simulate_keystrokes(window.into(), "cmd-v");
}
fn document(cx: &mut TestAppContext, window: WindowHandle<Workspace>) -> Entity<EditingDocument> {
    window
        .update(cx, |workspace, _, cx| {
            workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap()
                .view
                .editing_document(cx)
                .unwrap()
        })
        .unwrap()
}
fn finish_close_save(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
}

#[gpui::test]
fn toolbar_creation_uses_selected_folder_and_empty_or_escape_cancel(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let window = open(cx, Some(dir.path().to_owned()));
    click(cx, window, "tree-new-file");
    cx.simulate_keystrokes(window.into(), "enter");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "cancelled.txt");
    cx.simulate_keystrokes(window.into(), "escape");
    assert!(!dir.path().join("cancelled.txt").exists());
    click(cx, window, "tree-new-folder");
    paste(cx, window, "nested");
    cx.simulate_keystrokes(window.into(), "enter");
    assert!(dir.path().join("nested").is_dir());
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "file.txt");
    // Repeating New File while naming keeps the current filename.
    cx.simulate_keystrokes(window.into(), "cmd-n enter");
    assert!(dir.path().join("nested/file.txt").is_file());
    assert!(!dir.path().join("file.txt").exists());
    click(cx, window, "tree-collapse-all");
    window
        .update(cx, |workspace, window, cx| {
            let tree = workspace.tree.as_ref().unwrap();
            assert_eq!(tree.read(cx).watched_directories(), [dir.path().to_owned()]);
            assert!(tree.focus_handle(cx).is_focused(window));
            window.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn untitled_close_prompts_only_for_nonempty_contents(cx: &mut TestAppContext) {
    let window = open(cx, None);
    cx.simulate_keystrokes(window.into(), "cmd-n");
    let empty = document(cx, window);
    cx.update(|cx| assert!(empty.read(cx).is_untitled()));
    cx.simulate_keystrokes(window.into(), "cmd-w");
    assert!(!cx.has_pending_prompt());
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "draft");
    cx.simulate_keystrokes(window.into(), "cmd-w");
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    let draft = document(cx, window);
    cx.update(|cx| assert!(draft.read(cx).is_dirty()));
    cx.simulate_keystrokes(window.into(), "cmd-a backspace cmd-w");
    assert!(!cx.has_pending_prompt());
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "discard me");
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, _| {
            assert!(workspace.panes[&workspace.active_pane].tabs.is_empty());
            window.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn untitled_save_adopts_path_in_all_panes_and_keeps_undo(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.rs");
    let window = open(cx, Some(dir.path().to_owned()));
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "fn main() {}\n");
    let original = document(cx, window);
    cx.simulate_keystrokes(window.into(), "cmd-d cmd-n");
    assert_ne!(document(cx, window), original);
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.panes[&workspace.active_pane].tabs.len(), 2);
            assert_eq!(
                workspace
                    .panes
                    .values()
                    .map(|pane| pane.tabs.len())
                    .sum::<usize>(),
                3
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-w cmd-s");
    assert!(cx.did_prompt_for_new_path());
    assert!(!path.exists());
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
    let doc = document(cx, window);
    window
        .update(cx, |workspace, _, cx| {
            for pane in workspace.panes.values() {
                let tab = pane.active_tab().unwrap();
                assert_eq!(tab.path, path);
                assert_eq!(tab.title.as_ref(), "new.rs");
                assert!(!tab.view.is_dirty(cx));
                assert_eq!(tab.view.editing_document(cx).unwrap(), doc);
                match tab.view.status(cx) {
                    crate::tab_view::TabStatus::Text(status) => {
                        assert_eq!(status.language, Some("Rust"))
                    }
                    _ => panic!("expected text editor"),
                }
            }
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-z");
    cx.update(|cx| {
        assert!(doc.read(cx).is_dirty());
        assert_eq!(doc.read(cx).version().unwrap().text().len(), 0);
    });
    cx.simulate_keystrokes(window.into(), "cmd-s");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn first_save_refuses_an_open_destination_and_keeps_both_documents(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.txt");
    std::fs::write(&path, "original").unwrap();
    let window = open(cx, Some(dir.path().to_owned()));
    window
        .update(cx, |workspace, window, cx| {
            workspace.open_file(path.clone(), None, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    paste(cx, window, "unsaved existing edits");
    let existing = document(cx, window);
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "new draft");
    let draft = document(cx, window);
    cx.simulate_keystrokes(window.into(), "cmd-s");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(draft.read(cx).is_untitled());
        assert!(draft.read(cx).is_dirty());
        assert!(draft.read(cx).notice().unwrap().contains("already open"));
        assert!(existing.read(cx).is_dirty());
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn close_save_cancel_keeps_untitled_then_save_closes_the_renamed_tab(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.txt");
    let window = open(cx, Some(dir.path().to_owned()));
    cx.simulate_keystrokes(window.into(), "cmd-n");
    paste(cx, window, "keep this");
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    finish_close_save(cx);
    let doc = document(cx, window);
    cx.update(|cx| {
        assert!(doc.read(cx).is_untitled());
        assert!(doc.read(cx).is_dirty());
    });
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    finish_close_save(cx);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "keep this");
    window
        .update(cx, |workspace, window, _| {
            assert!(workspace.panes[&workspace.active_pane].tabs.is_empty());
            window.remove_window();
        })
        .unwrap();
}
