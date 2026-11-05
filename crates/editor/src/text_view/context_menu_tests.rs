use super::*;
use gpui::{TestAppContext, WindowHandle};
use plastext_buffer::OpenOptions;

fn open(cx: &mut TestAppContext, editable: bool) -> (tempfile::TempDir, WindowHandle<TextView>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.md");
    std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let (source, index) = plastext_buffer::open(&path, OpenOptions::default()).unwrap();
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut view = TextView::new(
            source,
            index,
            TextStyle {
                font_family: "Menlo".into(),
                font_size: px(14.),
                line_height: 1.4,
                tab_size: 4,
                sticky_scroll: false,
                max_parse_size: 0,
            },
            None,
            cx,
        );
        view.set_file_path(&path);
        if editable {
            let document = crate::EditingDocument::shared(path.clone(), OpenOptions::default(), cx);
            view.set_editing_document(document, cx);
            view.edit_by_default(cx);
        }
        view.focus_handle.focus(window);
        view
    });
    cx.run_until_parked();
    (dir, window)
}

fn select(cx: &mut TestAppContext, window: WindowHandle<TextView>, anchor: u64, head: u64) {
    window
        .update(cx, |view, _, cx| {
            view.selection = Some(Selection { anchor, head });
            view.selection_edited(cx);
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
}

fn right_click(
    cx: &mut TestAppContext,
    window: WindowHandle<TextView>,
    offset: u64,
) -> Point<Pixels> {
    // Paint the selection before hit-testing against its rows.
    let handle: gpui::AnyWindowHandle = window.into();
    handle
        .update(cx, |_, window, cx| window.draw(cx).clear())
        .unwrap();
    let position = window
        .update(cx, |view, _, _| {
            let caret = view.last_hits.as_ref().unwrap().caret(offset).unwrap();
            point(caret.left() + px(1.), caret.top() + caret.size.height / 2.)
        })
        .unwrap();
    gpui::VisualTestContext::from_window(window.into(), cx).simulate_event(MouseDownEvent {
        position,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(view.context_menu.is_some()))
        .unwrap();
    position
}

fn clipboard(cx: &TestAppContext) -> String {
    cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap())
}

#[gpui::test]
fn menu_preserves_reversed_selection_and_copies_all_reference_formats(cx: &mut TestAppContext) {
    let (_dir, window) = open(cx, true);
    select(cx, window, 13, 0);
    right_click(cx, window, 8);
    window
        .update(cx, |view, _, _| {
            let selection = view.selection.unwrap();
            assert_eq!((selection.anchor, selection.head), (13, 0));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "down down down down enter");
    assert_eq!(clipboard(cx), "file.md:1:1");
    right_click(cx, window, 8);
    cx.simulate_keystrokes(window.into(), "down down down down down enter");
    assert_eq!(clipboard(cx), "file.md:1-2");
    right_click(cx, window, 8);
    cx.simulate_keystrokes(window.into(), "up enter");
    assert_eq!(clipboard(cx), "in file.md:1-2:\nfirst\nsecond\n");
    right_click(cx, window, 16);
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.selection.unwrap().range(), 16..16)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "up enter");
    assert_eq!(clipboard(cx), "in file.md:3: third");
    window
        .update(cx, |view, window, _| {
            assert!(view.context_menu.is_none());
            assert!(view.focus_handle.is_focused(window));
            window.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn menu_cut_paste_undo_and_escape_use_the_editor_without_saving(cx: &mut TestAppContext) {
    let (dir, window) = open(cx, true);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("stale clipboard".into())));
    select(cx, window, 0, 13);
    cx.simulate_keystrokes(window.into(), "cmd-c cmd-v");
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.source.bytes(0..view.source.len()).unwrap().as_ref(),
                b"first\nsecond\nthird\n"
            );
        })
        .unwrap();
    select(cx, window, 0, 13);
    let position = right_click(cx, window, 8);
    // Click Cut in the menu; the underlying text must not receive this click.
    gpui::VisualTestContext::from_window(window.into(), cx)
        .simulate_click(position + point(px(24.), px(16.)), Default::default());
    cx.run_until_parked();
    assert_eq!(clipboard(cx), "first\nsecond\n");
    window
        .update(cx, |view, _, _| {
            assert!(view.context_menu.is_none());
            assert_eq!(
                view.source.bytes(0..view.source.len()).unwrap().as_ref(),
                b"third\n"
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-z cmd-up");
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("P".into())));
    right_click(cx, window, 6);
    cx.simulate_keystrokes(window.into(), "down enter"); // Paste is the first enabled item.
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.source.bytes(0..view.source.len()).unwrap().as_ref(),
                b"first\nPsecond\nthird\n"
            );
        })
        .unwrap();
    right_click(cx, window, 6);
    cx.simulate_keystrokes(window.into(), "escape");
    window
        .update(cx, |view, window, _| {
            assert!(view.context_menu.is_none());
            assert!(view.is_editing());
            assert!(view.focus_handle.is_focused(window));
            window.remove_window();
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.md")).unwrap(),
        "first\nsecond\nthird\n"
    );
}

#[gpui::test]
fn read_only_menu_disables_mutations_and_newer_copy_requests_win(cx: &mut TestAppContext) {
    let (_dir, window) = open(cx, false);
    select(cx, window, 0, 5);
    right_click(cx, window, 3);
    window
        .update(cx, |view, window, cx| {
            let enabled = view.context_menu_options();
            assert!(!enabled[0] && !enabled[2]);
            assert!(enabled[1] && enabled[3] && enabled[4] && enabled[5]);
            view.context_menu_command(0, window, cx); // Disabled Cut must do nothing.
            assert!(view.context_menu.is_some());
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "down enter");
    assert_eq!(clipboard(cx), "first");
    window
        .update(cx, |view, _, cx| {
            view.copy_text(Format::WithLocation, cx);
            view.selection = Some(Selection {
                anchor: 6,
                head: 12,
            });
            view.copy_text(Format::Text, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(clipboard(cx), "second");
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}
