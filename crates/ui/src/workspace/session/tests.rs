use super::*;
use gpui::{ClipboardItem, TestAppContext, VisualTestContext, WindowHandle, size};
use settings::session::{Sessions, ViewMode, ViewState};

fn settle(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    for _ in 0..4 {
        cx.refresh().unwrap();
        cx.run_until_parked();
    }
}
fn open(cx: &mut TestAppContext, root: &Path, session: Option<Session>) -> WindowHandle<Workspace> {
    let window = cx.add_window(|window, cx| {
        let mut ws = Workspace::new(Some(root.to_owned()), Arc::default(), window, cx);
        if let Some(session) = session {
            ws.restore_session(session, window, cx);
        }
        ws
    });
    VisualTestContext::from_window(window.into(), cx).simulate_resize(size(px(1500.), px(800.)));
    window
}
fn snapshot(cx: &mut TestAppContext, w: WindowHandle<Workspace>) -> Session {
    w.update(cx, |ws, _, cx| ws.session_snapshot(cx).unwrap().1)
        .unwrap()
}
fn saved_tab(root: &Path, name: &str, mode: ViewMode) -> SessionTab {
    SessionTab {
        path: root.join(name),
        preview: false,
        view: ViewState {
            mode,
            ..Default::default()
        },
    }
}
fn one_pane(tabs: Vec<SessionTab>) -> Session {
    Session {
        layout: SessionLayout::Pane(0),
        panes: vec![SessionPane {
            active: (!tabs.is_empty()).then_some(0),
            tabs,
        }],
        active_pane: 0,
        sidebar_width: 310.,
        zoom: 110,
        sync_scroll: false,
    }
}

#[gpui::test]
fn folder_session_restores_splits_modes_positions_and_lazy_tabs(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Heading\n\nA paragraph of text with enough words to wrap across several lines in a split pane.\n\n".repeat(100)).unwrap();
    let first = open(cx, dir.path(), None);
    first
        .update(cx, |ws, w, cx| ws.open_file(path.clone(), None, w, cx))
        .unwrap();
    settle(cx);
    first
        .update(cx, |ws, w, cx| ws.split(Axis::Horizontal, w, cx))
        .unwrap();
    settle(cx);
    first
        .update(cx, |ws, w, cx| {
            ws.sidebar_width = px(290.);
            ws.set_zoom(Zoom::from_percent(90), cx);
            let Layout::Split(_, children) = &mut ws.layout else {
                panic!()
            };
            children[0].weight = 2.;
            children[1].weight = 3.;
            for (pane, mode) in [(0, ViewMode::Rendered), (1, ViewMode::Source)] {
                let view = ws.panes[&pane].active_tab().unwrap().view.clone();
                view.restore_session(
                    &ViewState {
                        mode,
                        offset: 2800,
                        ..Default::default()
                    },
                    w,
                    cx,
                );
            }
        })
        .unwrap();
    settle(cx);
    let mut expected = snapshot(cx, first);
    assert_eq!(expected.panes[0].tabs[0].view.mode, ViewMode::Rendered);
    assert_eq!(expected.panes[1].tabs[0].view.mode, ViewMode::Source);
    assert!(expected.panes.iter().all(|p| p.tabs[0].view.offset > 1000));
    // The inactive tab is deliberately absent. Restoring headers must not open it.
    expected.panes[0]
        .tabs
        .push(saved_tab(dir.path(), "not-loaded.txt", ViewMode::Source));
    first.update(cx, |_, w, _| w.remove_window()).unwrap();
    let file = dir.path().join("state/sessions.json");
    Sessions::save(&file, vec![(dir.path().to_owned(), expected.clone())]).unwrap();
    let restored = open(cx, dir.path(), Sessions::load(&file, dir.path()).unwrap());
    settle(cx);
    let actual = snapshot(cx, restored);
    assert_eq!(actual.layout, expected.layout);
    assert_eq!(actual.active_pane, expected.active_pane);
    assert_eq!(actual.sidebar_width, expected.sidebar_width);
    assert_eq!(actual.zoom, expected.zoom);
    for (actual, expected) in actual.panes.iter().zip(&expected.panes) {
        assert_eq!(actual.active, expected.active);
        assert_eq!(actual.tabs.len(), expected.tabs.len());
        for (actual, expected) in actual.tabs.iter().zip(&expected.tabs) {
            assert_eq!(actual.path, expected.path);
            assert_eq!(actual.view.mode, expected.view.mode);
            assert_eq!(actual.view.offset, expected.view.offset);
            if let (Some(a), Some(b)) = (actual.view.scroll, expected.view.scroll) {
                assert_eq!(a.source.start, b.source.start);
                assert!((a.source.fraction - b.source.fraction).abs() < 0.01);
            }
        }
    }
    restored
        .update(cx, |ws, _, cx| {
            let TabView::Deferred(tab) = &ws.panes[&0].tabs[1].view else {
                panic!("inactive tab was loaded")
            };
            assert!(tab.read(cx).error.is_none());
            assert!(!ws.panes[&0].opening.is_pending());
        })
        .unwrap();
    // Selecting the unavailable file gives a retryable tab, without dropping peers.
    restored
        .update(cx, |ws, w, cx| ws.activate(0, 1, w, cx))
        .unwrap();
    settle(cx);
    restored
        .update(cx, |ws, _, cx| {
            let TabView::Deferred(tab) = &ws.panes[&0].tabs[1].view else {
                panic!()
            };
            assert!(tab.read(cx).error.is_some());
            assert_eq!(ws.panes[&1].tabs.len(), 1);
        })
        .unwrap();
    std::fs::write(dir.path().join("not-loaded.txt"), "Now available\n").unwrap();
    restored
        .update(cx, |ws, w, cx| ws.activate(0, 1, w, cx))
        .unwrap();
    settle(cx);
    restored
        .update(cx, |ws, w, _| {
            assert!(matches!(ws.panes[&0].tabs[1].view, TabView::Text(_)));
            w.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn folder_session_close_writes_only_after_confirmation_and_never_saves_contents(
    cx: &mut TestAppContext,
) {
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("config/sessions.json");
    cx.update(|cx| cx.set_global(SessionStorage(Some(storage.clone()))));
    let root = dir.path().join("folder");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("notes.txt");
    std::fs::write(&path, "Original\n").unwrap();
    let window = open(cx, &root, None);
    window
        .update(cx, |ws, w, cx| ws.open_file(path.clone(), None, w, cx))
        .unwrap();
    settle(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("Unsaved content".into())));
    cx.simulate_keystrokes(window.into(), "cmd-a cmd-v");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(60));
    cx.run_until_parked();
    assert!(!storage.exists(), "there must be no idle/checkpoint writes");
    window
        .update(cx, |ws, w, cx| {
            ws.should_close_window(w, cx);
        })
        .unwrap();
    cx.simulate_prompt_answer("Cancel");
    settle(cx);
    assert!(!storage.exists());
    window
        .update(cx, |ws, w, cx| {
            ws.should_close_window(w, cx);
        })
        .unwrap();
    cx.simulate_prompt_answer("Discard");
    settle(cx);
    assert!(
        window.read_with(cx, |_, _| ()).is_err(),
        "approved window should close"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Original\n");
    let saved = Sessions::load(&storage, &root).unwrap().unwrap();
    assert_eq!(saved.panes[0].tabs[0].path, path);
    assert!(
        !std::fs::read_to_string(&storage)
            .unwrap()
            .contains("Unsaved content")
    );
}

#[gpui::test]
fn folder_session_closed_loading_tab_does_not_reappear_and_hidden_state_survives(
    cx: &mut TestAppContext,
) {
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("first.txt"), "first").unwrap();
    std::fs::write(dir.path().join("second.tex"), "\\section{Second}\n").unwrap();
    let mut saved = one_pane(vec![
        saved_tab(dir.path(), "first.txt", ViewMode::Source),
        saved_tab(dir.path(), "second.tex", ViewMode::Rendered),
    ]);
    saved.panes[0].tabs[1].view.offset = 23;
    let window = open(cx, dir.path(), Some(saved.clone()));
    window
        .update(cx, |ws, w, cx| {
            ws.close_tab_now(0, 0, w, cx);
            // The newly selected tab loads; close it before that callback can finish too.
            ws.close_tab_now(0, 0, w, cx);
        })
        .unwrap();
    settle(cx);
    assert!(snapshot(cx, window).panes[0].tabs.is_empty());
    window.update(cx, |_, w, _| w.remove_window()).unwrap();
    let window = open(cx, dir.path(), Some(saved.clone()));
    settle(cx);
    let actual = snapshot(cx, window);
    assert_eq!(actual.panes[0].tabs[1], saved.panes[0].tabs[1]);
    window.update(cx, |_, w, _| w.remove_window()).unwrap();
}

#[gpui::test]
fn folder_session_tex_keeps_independent_source_and_rendered_modes(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "\\title{{A title}}\n\\author{{An author}}\n\\begin{{document}}\n\\maketitle\n{}\\end{{document}}\n",
        "\\section{Heading}\nA paragraph of text.\n\n".repeat(100)
    );
    std::fs::write(dir.path().join("paper.tex"), &text).unwrap();
    let mut saved = one_pane(vec![saved_tab(dir.path(), "paper.tex", ViewMode::Rendered)]);
    saved.panes.push(SessionPane {
        tabs: vec![saved_tab(dir.path(), "paper.tex", ViewMode::Source)],
        active: Some(0),
    });
    saved.active_pane = 1;
    saved.layout = SessionLayout::Split {
        vertical: true,
        children: vec![
            SessionChild {
                layout: SessionLayout::Pane(0),
                weight: 0.4,
            },
            SessionChild {
                layout: SessionLayout::Pane(1),
                weight: 0.6,
            },
        ],
    };
    for pane in &mut saved.panes {
        pane.tabs[0].view.offset = 1800;
    }
    let first = open(cx, dir.path(), Some(saved));
    settle(cx);
    let expected = snapshot(cx, first);
    assert_eq!(expected.panes[0].tabs[0].view.mode, ViewMode::Rendered);
    assert_eq!(expected.panes[1].tabs[0].view.mode, ViewMode::Source);
    assert!(expected.panes.iter().all(|p| p.tabs[0].view.offset > 1000));
    first.update(cx, |_, w, _| w.remove_window()).unwrap();
    let second = open(cx, dir.path(), Some(expected.clone()));
    settle(cx);
    let actual = snapshot(cx, second);
    assert_eq!(actual, expected);
    second
        .update(cx, |ws, w, cx| {
            let view = &ws.panes[&1].tabs[0].view;
            assert!(
                view.focus_handle(cx).is_focused(w),
                "restored Source must keep editor focus"
            );
            w.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn folder_session_quit_batches_all_folder_windows_and_excludes_untitled(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("state/sessions.json");
    cx.update(|cx| cx.set_global(SessionStorage(Some(storage.clone()))));
    let windows: Vec<_> = (0..2)
        .map(|i| {
            let root = dir.path().join(format!("folder-{i}"));
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("notes.md"), "# Notes\n").unwrap();
            let w = open(
                cx,
                &root,
                Some(one_pane(vec![saved_tab(
                    &root,
                    "notes.md",
                    ViewMode::Rendered,
                )])),
            );
            (root, w)
        })
        .collect();
    settle(cx);
    windows[0]
        .1
        .update(cx, |ws, w, cx| {
            ws.new_file(&NewFile, w, cx);
        })
        .unwrap();
    settle(cx);
    windows[0]
        .1
        .update(cx, |ws, w, cx| {
            assert_eq!(ws.panes[&0].tabs.len(), 2);
            ws.confirm_close(super::super::editing::CloseTarget::Quit, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    for (root, w) in windows {
        let saved = Sessions::load(&storage, &root).unwrap().unwrap();
        assert_eq!(saved.panes[0].tabs.len(), 1);
        assert_eq!(saved.panes[0].tabs[0].path, root.join("notes.md"));
        w.update(cx, |_, w, _| w.remove_window()).unwrap();
    }
    cx.update(|cx| assert!(!closing(cx)));
}

#[gpui::test]
fn folder_session_close_rechecks_work_started_during_the_metadata_write(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("state/sessions.json");
    cx.update(|cx| cx.set_global(SessionStorage(Some(storage.clone()))));
    std::fs::write(dir.path().join("notes.txt"), "Original\n").unwrap();
    let w = open(
        cx,
        dir.path(),
        Some(one_pane(vec![saved_tab(
            dir.path(),
            "notes.txt",
            ViewMode::Source,
        )])),
    );
    settle(cx);
    let doc = w
        .update(cx, |ws, w, cx| {
            let doc = ws.panes[&0].tabs[0].view.editing_document(cx).unwrap();
            ws.should_close_window(w, cx);
            doc.update(cx, |d, cx| d.set_file_operation_pending(true, cx));
            doc
        })
        .unwrap();
    settle(cx);
    w.read_with(cx, |ws, cx| {
        assert!(
            ws.notice
                .as_ref()
                .is_some_and(|n| n.contains("changed while closing"))
        );
        assert!(!closing(cx));
    })
    .unwrap();
    w.update(cx, |ws, w, cx| {
        doc.update(cx, |d, cx| d.set_file_operation_pending(false, cx));
        ws.should_close_window(w, cx);
    })
    .unwrap();
    settle(cx);
    assert!(w.read_with(cx, |_, _| ()).is_err());
}
