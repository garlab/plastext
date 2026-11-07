use super::*;
use gpui::{ClipboardItem, TestAppContext, WindowHandle};

fn check(window: WindowHandle<Workspace>, cx: &mut TestAppContext) {
    window
        .update(cx, |workspace, _, cx| {
            workspace.observe_disk(
                Batch {
                    rescan: true,
                    ..Batch::default()
                },
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
}

#[gpui::test]
fn dirty_file_banner_keeps_edits_and_save_requires_an_explicit_overwrite(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "original").unwrap();
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace = Workspace::new(None, Arc::default(), window, cx);
        workspace.open_with_kind(path.clone(), None, OpenKind::Keep, window, cx);
        workspace
    });
    cx.run_until_parked();
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("my edits".into())));
    cx.simulate_keystrokes(window.into(), "cmd-a cmd-v");
    std::fs::write(&path, "outside").unwrap();
    check(window, cx);
    let document = window
        .update(cx, |workspace, _, cx| {
            let doc = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap()
                .view
                .editing_document(cx)
                .unwrap();
            assert!(doc.read(cx).is_dirty());
            assert_eq!(doc.read(cx).disk_change(), Some(DiskChange::Changed));
            doc
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let compare = visual.debug_bounds("disk-diff").unwrap();
    visual.simulate_click(compare.center(), Default::default());
    visual.run_until_parked();
    let comparison = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|w| w.downcast::<Comparison>())
            .unwrap()
    });
    cx.simulate_keystrokes(comparison.into(), "cmd-a cmd-c");
    let copied = cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap());
    assert!(copied.contains("-outside"));
    assert!(copied.contains("+my edits"));
    cx.simulate_keystrokes(comparison.into(), "cmd-w");
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let keep = visual.debug_bounds("disk-keep").unwrap();
    visual.simulate_click(keep.center(), Default::default());
    visual.run_until_parked();
    cx.simulate_keystrokes(window.into(), "cmd-s");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "outside");
    cx.update(|cx| assert!(document.read(cx).confirming_overwrite()));
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let overwrite = visual.debug_bounds("disk-overwrite").unwrap();
    visual.simulate_click(overwrite.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "my edits");
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn markdown_preview_refreshes_after_an_external_write(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.md");
    std::fs::write(&path, "# Original\n").unwrap();
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace = Workspace::new(None, Arc::default(), window, cx);
        workspace.open_with_kind(path.clone(), None, OpenKind::Keep, window, cx);
        workspace
    });
    cx.run_until_parked();
    check(window, cx); // Establish the read-only preview's initial disk version.
    std::fs::write(&path, "# Outside update\n").unwrap();
    check(window, cx);
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, cx| {
            let tab = workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap();
            assert!(
                tab.view
                    .symbols(cx)
                    .unwrap()
                    .iter()
                    .any(|s| s.name == "Outside update")
            );
            assert!(!tab.view.is_dirty(cx));
            window.remove_window();
        })
        .unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "# Outside update\n");
}
