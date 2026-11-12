use super::*;
use gpui::{
    ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, WindowHandle, point, size,
};

// GPUI's test platform does not schedule display ticks. Flush a few bounded
// frames so offscreen list rows can be measured before applying their fraction.
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

fn open(cx: &mut TestAppContext, path: &Path) -> WindowHandle<Workspace> {
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace = Workspace::new(None, Arc::default(), window, cx);
        workspace.open_file(path.to_owned(), None, window, cx);
        workspace
    });
    VisualTestContext::from_window(window.into(), cx).simulate_resize(size(px(1500.), px(800.)));
    settle(cx);
    window
}

fn split(cx: &mut TestAppContext, window: WindowHandle<Workspace>) {
    window
        .update(cx, |ws, window, cx| ws.split(Axis::Horizontal, window, cx))
        .unwrap();
    settle(cx);
}

fn toggle(cx: &mut TestAppContext, window: WindowHandle<Workspace>) {
    window
        .update(cx, |_, window, cx| {
            assert!(window.is_action_available(&ToggleScrollSync, cx));
            window.dispatch_action(Box::new(ToggleScrollSync), cx);
        })
        .unwrap();
    settle(cx);
}

fn get_views(cx: &mut TestAppContext, window: WindowHandle<Workspace>) -> Vec<TabView> {
    window
        .update(cx, |ws, _, _| {
            (0..ws.next_pane)
                .filter_map(|id| ws.panes.get(&id)?.active_tab().map(|tab| tab.view.clone()))
                .collect()
        })
        .unwrap()
}

#[track_caller]
fn position(cx: &TestAppContext, view: &TabView) -> Position {
    cx.update(|cx| {
        view.scroll_position(cx)
            .expect("view has a measured scroll position")
    })
}

fn scroll(cx: &mut TestAppContext, window: WindowHandle<Workspace>, pane: usize, offset: u64) {
    window
        .update(cx, |ws, _, cx| {
            ws.lead_scroll(pane, cx);
            ws.panes[&pane]
                .active_tab()
                .unwrap()
                .view
                .scroll_to_offset(offset, cx);
        })
        .unwrap();
    settle(cx);
}

fn wheel(cx: &mut TestAppContext, window: WindowHandle<Workspace>, pane: &'static str) {
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let bounds = visual.debug_bounds(pane).unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: bounds.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-147.))),
        ..Default::default()
    });
    settle(cx);
}

#[gpui::test]
fn scroll_sync_is_opt_in_bidirectional_and_links_three_panes_without_focus_changes(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.txt");
    std::fs::write(&path, "a line of source text\n".repeat(1000)).unwrap();
    let window = open(cx, &path);
    split(cx, window);
    split(cx, window);
    let views = get_views(cx, window);
    assert!(
        !window
            .update(cx, |ws, _, _| ws.scroll_sync.enabled)
            .unwrap()
    );
    scroll(cx, window, 0, 2200);
    assert_eq!(position(cx, &views[0]).source.start, 2200);
    assert_eq!(position(cx, &views[1]).source.start, 0);
    assert_eq!(position(cx, &views[2]).source.start, 0);
    toggle(cx, window);
    scroll(cx, window, 0, 4400);
    assert_eq!(position(cx, &views[0]), position(cx, &views[1]));
    assert_eq!(position(cx, &views[0]), position(cx, &views[2]));
    let focus = window
        .update(cx, |_, window, cx| {
            views[2].focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(focus);
    wheel(cx, window, "pane-0");
    assert!(position(cx, &views[0]).source.start > 4400);
    assert_eq!(position(cx, &views[0]), position(cx, &views[1]));
    assert_eq!(position(cx, &views[0]), position(cx, &views[2]));
    assert!(
        window
            .update(cx, |_, window, cx| views[2]
                .focus_handle(cx)
                .is_focused(window))
            .unwrap()
    );
    wheel(cx, window, "pane-1");
    assert_eq!(position(cx, &views[0]), position(cx, &views[1]));
    assert_eq!(position(cx, &views[0]), position(cx, &views[2]));
    cx.simulate_keystrokes(window.into(), "cmd-down");
    settle(cx);
    assert!(position(cx, &views[2]).source.start > 10_000);
    assert_eq!(position(cx, &views[0]), position(cx, &views[2]));
    assert_eq!(position(cx, &views[1]), position(cx, &views[2]));
    scroll(cx, window, 1, 4400);
    let before = position(cx, &views[0]);
    toggle(cx, window);
    wheel(cx, window, "pane-1");
    assert_eq!(position(cx, &views[0]), before);
    assert_ne!(position(cx, &views[1]), before);
}

#[gpui::test]
fn scroll_sync_leaves_other_files_and_hidden_tabs_alone(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("first.txt");
    let other = dir.path().join("other.txt");
    let content = "a line of source text\n".repeat(1000);
    std::fs::write(&path, &content).unwrap();
    std::fs::write(&other, &content).unwrap();
    let window = open(cx, &path);
    split(cx, window);
    split(cx, window);
    let hidden = get_views(cx, window)[2].clone();
    window
        .update(cx, |ws, window, cx| ws.open_file(other, None, window, cx))
        .unwrap();
    settle(cx);
    let views = get_views(cx, window);
    toggle(cx, window);
    scroll(cx, window, 0, 4400);
    assert_eq!(position(cx, &views[0]), position(cx, &views[1]));
    assert_eq!(position(cx, &views[2]).source.start, 0);
    assert_eq!(position(cx, &hidden).source.start, 0);
}

#[gpui::test]
fn scroll_sync_links_source_and_native_readers_in_both_directions(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    for extension in ["md", "tex"] {
        let path = dir.path().join(format!("document.{extension}"));
        let content: String = (0..200).map(|i| if extension == "md" {
            format!("## Section {i}\n\nA paragraph of readable text with enough words to wrap.\n\n")
        } else {
            format!("\\section{{Section {i}}}\n\nA paragraph of readable text with enough words to wrap.\n\n")
        }).collect();
        std::fs::write(&path, &content).unwrap();
        let window = open(cx, &path);
        split(cx, window);
        split(cx, window);
        let views = get_views(cx, window);
        window
            .update(cx, |ws, window, cx| {
                match &views[0] {
                    TabView::Markdown(view) => {
                        view.update(cx, |view, cx| view.show_source(window, cx))
                    }
                    TabView::Tex(view) => view.update(cx, |view, cx| {
                        view.set_mode(crate::tex_view::Mode::Source, window, cx)
                    }),
                    _ => unreachable!(),
                }
                ws.focus_pane(0, cx);
            })
            .unwrap();
        settle(cx);
        toggle(cx, window);
        let target = content.find("A paragraph").unwrap() + content.find("Section 80").unwrap();
        scroll(cx, window, 0, target as u64);
        let source = position(cx, &views[0]).source;
        for view in &views[1..] {
            let rendered = position(cx, view).source;
            assert!(
                rendered.start.abs_diff(source.start) < 100,
                "{extension}: {source:?} -> {rendered:?}"
            );
        }
        wheel(cx, window, "pane-1");
        let leader = position(cx, &views[1]);
        let follower = position(cx, &views[2]);
        assert_eq!(
            leader.rendered.unwrap().span.start,
            follower.rendered.unwrap().span.start
        );
        assert!(
            (leader.rendered.unwrap().span.fraction - follower.rendered.unwrap().span.fraction)
                .abs()
                < 0.01
        );
        assert!(
            leader
                .source
                .offset()
                .abs_diff(position(cx, &views[0]).source.offset())
                < 2,
            "{extension}: {leader:?} -> {:?}",
            position(cx, &views[0])
        );
        // Switching a follower's mode re-applies the current source anchor.
        window
            .update(cx, |_, window, cx| match &views[2] {
                TabView::Markdown(view) => view.update(cx, |view, cx| view.show_source(window, cx)),
                TabView::Tex(view) => view.update(cx, |view, cx| {
                    view.set_mode(crate::tex_view::Mode::Source, window, cx)
                }),
                _ => unreachable!(),
            })
            .unwrap();
        settle(cx);
        assert!(
            leader
                .source
                .offset()
                .abs_diff(position(cx, &views[2]).source.offset())
                < 2
        );
        window
            .update(cx, |_, window, cx| match &views[2] {
                TabView::Markdown(view) => {
                    view.update(cx, |view, cx| view.set_rendered(true, window, cx))
                }
                TabView::Tex(view) => view.update(cx, |view, cx| {
                    view.set_mode(crate::tex_view::Mode::Rendered, window, cx)
                }),
                _ => unreachable!(),
            })
            .unwrap();
        settle(cx);
        assert_eq!(
            leader.rendered.unwrap().span.start,
            position(cx, &views[2]).rendered.unwrap().span.start
        );
        let before_split = position(cx, &views[1]).rendered.unwrap().span.start;
        window
            .update(cx, |ws, window, cx| {
                ws.focus_pane(1, cx);
                views[1].focus_handle(cx).focus(window);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-d");
        settle(cx);
        let fourth = get_views(cx, window).pop().unwrap();
        let original = position(cx, &views[1]).rendered.unwrap();
        let duplicate = position(cx, &fourth).rendered.unwrap();
        assert_eq!(original.span.start, before_split);
        assert_eq!(original.span.start, duplicate.span.start);
        assert!((original.span.fraction - duplicate.span.fraction).abs() < 0.01);
        window
            .update(cx, |_, window, cx| views[1].focus_handle(cx).focus(window))
            .unwrap();
        cx.simulate_keystrokes(window.into(), "g");
        settle(cx);
        let end = position(cx, &views[1]);
        assert_eq!(end.source.start, 0, "{extension}: {end:?}");
        assert!(
            end.source
                .offset()
                .abs_diff(position(cx, &views[0]).source.offset())
                < 2
        );
    }
}
