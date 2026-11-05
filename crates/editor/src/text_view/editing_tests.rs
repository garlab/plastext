use super::*;
use gpui::TestAppContext;
use plastext_buffer::{FileDocument, OpenOptions};

fn setup(
    cx: &TestAppContext,
    bytes: &[u8],
    chunked: bool,
) -> (tempfile::TempDir, Entity<TextView>, Entity<TextView>) {
    setup_with_ready(cx, bytes, chunked, true)
}
fn setup_with_ready(
    cx: &TestAppContext,
    bytes: &[u8],
    chunked: bool,
    ready: bool,
) -> (tempfile::TempDir, Entity<TextView>, Entity<TextView>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edit.txt");
    std::fs::write(&path, bytes).unwrap();
    let options = if chunked {
        OpenOptions {
            large_file_threshold: 1,
            block_cache: 8 * 1024 * 1024,
        }
    } else {
        OpenOptions::default()
    };
    let file = FileDocument::open(&path, options, &AtomicBool::new(false), |_, _| {}).unwrap();
    let (source, index) = plastext_buffer::open(&path, OpenOptions::default()).unwrap();
    let (a, b) = cx.update(|cx| {
        let document = EditingDocument::shared(path, options, cx);
        document.update(cx, |doc, _| {
            doc.file = ready.then_some(file);
            doc.active = ready;
        });
        let style = TextStyle {
            font_family: "Menlo".into(),
            font_size: px(14.),
            line_height: 1.4,
            tab_size: 4,
            sticky_scroll: false,
            max_parse_size: 0,
        };
        let make = |cx: &mut App| {
            cx.new(|cx| {
                let mut view =
                    TextView::new(source.clone(), index.clone(), style.clone(), None, cx);
                view.set_editing_document(document.clone(), cx);
                view
            })
        };
        (make(cx), make(cx))
    });
    (dir, a, b)
}
fn insert(text: &str) -> Input {
    Input::Text {
        text: text.into(),
        range: None,
        marked: false,
        selected: None,
        separate: false,
    }
}
fn send(cx: &TestAppContext, view: &Entity<TextView>, input: Input) {
    cx.update(|cx| view.update(cx, |view, cx| view.queue_input(input, cx)));
}
fn content(cx: &TestAppContext, view: &Entity<TextView>) -> String {
    cx.update(|cx| {
        let source = &view.read(cx).source;
        input_text(&source.bytes(0..source.len()).unwrap())
    })
}

#[gpui::test]
fn split_panes_share_edits_and_history(cx: &mut TestAppContext) {
    let (_dir, a, b) = setup(cx, b"abc\n", false);
    send(cx, &b, Input::Selection(BufferSelection::caret(3)));
    send(cx, &a, insert("x"));
    send(cx, &a, insert("y"));
    assert_eq!(content(cx, &b), "xyabc\n");
    cx.update(|cx| {
        assert_eq!(a.read(cx).selection.unwrap().head, 2);
        assert_eq!(b.read(cx).selection.unwrap().head, 5);
        assert!(a.read(cx).is_dirty());
        assert_eq!(
            a.read(cx)
                .editing_document()
                .unwrap()
                .read(cx)
                .pending_inputs,
            0
        );
    });
    send(cx, &b, Input::Undo);
    assert_eq!(content(cx, &a), "abc\n");
    cx.update(|cx| assert!(!a.read(cx).is_dirty()));
    send(cx, &a, Input::Redo);
    assert_eq!(content(cx, &b), "xyabc\n");
}

#[gpui::test]
fn composition_is_one_undo_transaction_and_handles_utf16(cx: &mut TestAppContext) {
    let (_dir, a, _b) = setup(cx, b"tail", false);
    for text in ["n", "ni", "你😀"] {
        send(
            cx,
            &a,
            Input::Text {
                text: text.into(),
                range: None,
                marked: true,
                selected: Some(text.encode_utf16().count()..text.encode_utf16().count()),
                separate: false,
            },
        );
    }
    send(cx, &a, insert("你😀"));
    assert_eq!(content(cx, &a), "你😀tail");
    send(cx, &a, Input::Undo);
    assert_eq!(content(cx, &a), "tail");
    send(cx, &a, Input::Redo);
    send(
        cx,
        &a,
        Input::Text {
            text: "!".into(),
            range: Some(1..3),
            marked: false,
            selected: None,
            separate: true,
        },
    );
    assert_eq!(content(cx, &a), "你!tail");
}

#[gpui::test]
fn newline_eof_and_grapheme_deletion(cx: &mut TestAppContext) {
    let (_dir, a, _b) = setup(cx, "a\r\nb😀".as_bytes(), false);
    send(cx, &a, Input::Selection(BufferSelection::caret(8)));
    send(
        cx,
        &a,
        Input::Cursor(CursorCommand::Delete {
            direction: Direction::Backward,
        }),
    );
    assert_eq!(content(cx, &a), "a\r\nb");
    send(cx, &a, Input::Newline);
    assert_eq!(content(cx, &a), "a\r\nb\r\n");
    send(cx, &a, insert("c"));
    send(cx, &a, Input::Vertical(-1, false));
    cx.update(|cx| assert_eq!(a.read(cx).selection.unwrap().head, 4));
}

#[gpui::test]
fn uncached_input_retries_and_stale_composition_is_rejected(cx: &mut TestAppContext) {
    let bytes = vec![b'a'; 512 * 1024];
    let (_dir, a, b) = setup(cx, &bytes, true);
    send(cx, &a, insert("!"));
    cx.executor().run_until_parked();
    cx.update(|cx| {
        assert!(!a.read(cx).has_pending_input());
        assert_eq!(a.read(cx).source.bytes(0..2).unwrap().as_ref(), b"!a");
        // Simulate a delayed explicit IME replacement from before another pane's edit.
        a.update(cx, |view, cx| {
            let editing = view.editing.as_mut().unwrap();
            editing.queue.push_back(QueuedInput {
                input: Arc::new(Input::Text {
                    text: "wrong".into(),
                    range: Some(0..1),
                    marked: false,
                    selected: None,
                    separate: true,
                }),
                coordinates: editing.generation,
            });
            editing
                .document
                .update(cx, |doc, _| doc.pending_inputs += 1);
        });
    });
    send(cx, &b, insert("?"));
    cx.executor().run_until_parked();
    cx.update(|cx| a.update(cx, |view, cx| view.drain_input(cx)));
    cx.update(|cx| {
        let view = a.read(cx);
        assert!(!view.has_pending_input());
        assert!(
            !view
                .source
                .bytes(0..10)
                .unwrap()
                .windows(5)
                .any(|part| part == b"wrong")
        );
        assert!(view.notice.as_ref().unwrap().contains("document changed"));
    });
}

#[test]
fn input_projection_preserves_invalid_byte_coordinates() {
    let bytes = b"a\xf0\x9f\x98z";
    assert_eq!(input_text(bytes).encode_utf16().count(), bytes.len());
}

#[gpui::test]
fn dispatched_keys_use_edit_context_and_reveal_the_caret(cx: &mut TestAppContext) {
    let (_dir, a, b) = setup(cx, b"", false);
    cx.update(crate::init);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            a.read(cx).focus_handle.focus(window);
            a.clone()
        })
        .unwrap()
    });
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "h e l l o enter w o r l d");
    assert_eq!(content(cx, &b), "hello\nworld");
    cx.simulate_keystrokes(window.into(), "cmd-up shift-right shift-right backspace");
    assert_eq!(content(cx, &b), "llo\nworld");
    cx.simulate_keystrokes(window.into(), "cmd-z");
    assert_eq!(content(cx, &b), "hello\nworld");
    cx.simulate_keystrokes(window.into(), "cmd-down");
    send(cx, &a, insert(&"x".repeat(1000)));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = a.read(cx);
        assert!(view.scroll_x > px(0.));
        let hits = view.last_hits.as_ref().unwrap();
        let caret = hits.caret(view.selection.unwrap().head).unwrap();
        assert!(caret.right() <= hits.bounds.right());
    });
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn save_completion_keeps_later_typing_dirty_and_conflicts_keep_disk(cx: &mut TestAppContext) {
    let (dir, a, b) = setup(cx, b"abc", false);
    let path = dir.path().join("edit.txt");
    send(cx, &a, insert("1"));
    send(cx, &a, Input::Save);
    send(cx, &a, insert("2"));
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), b"1abc");
    cx.update(|cx| assert!(b.read(cx).is_dirty()));
    send(cx, &a, Input::Save);
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), b"12abc");
    cx.update(|cx| assert!(!b.read(cx).is_dirty()));
    std::fs::write(&path, b"external").unwrap();
    send(cx, &b, insert("3"));
    send(cx, &b, Input::Save);
    cx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), b"external");
    cx.update(|cx| {
        assert!(a.read(cx).is_dirty());
        assert!(
            a.read(cx)
                .notice
                .as_ref()
                .unwrap()
                .contains("Couldn't save")
        );
    });
}

#[gpui::test]
fn reentering_edit_mode_uses_the_read_selection(cx: &mut TestAppContext) {
    let (_dir, a, _b) = setup(cx, b"abcdef", false);
    cx.update(|cx| {
        a.read(cx)
            .editing_document()
            .unwrap()
            .update(cx, |doc, cx| doc.leave_edit(cx));
    });
    cx.update(|cx| {
        a.update(cx, |view, cx| {
            assert!(!view.is_editing());
            view.selection = Some(Selection { anchor: 2, head: 4 });
            view.begin_edit(cx);
        })
    });
    send(cx, &a, insert("!"));
    assert_eq!(content(cx, &a), "ab!ef");
}

#[gpui::test]
fn typing_during_preparation_waits_for_the_shared_original(cx: &mut TestAppContext) {
    let (_dir, a, b) = setup_with_ready(cx, b"abc", false, false);
    cx.update(|cx| a.update(cx, |view, cx| view.begin_edit(cx)));
    send(cx, &a, insert("1"));
    send(cx, &a, insert("2"));
    cx.update(|cx| assert!(a.read(cx).has_pending_input()));
    cx.run_until_parked();
    assert_eq!(content(cx, &b), "12abc");
    cx.update(|cx| {
        assert!(!a.read(cx).has_pending_input());
        assert!(b.read(cx).is_dirty());
    });
}

#[gpui::test]
fn a_delayed_cursor_resumes_against_another_panes_edit(cx: &mut TestAppContext) {
    let text = format!("a{}b", "\u{0301}".repeat(10_000));
    let (_dir, a, b) = setup(cx, text.as_bytes(), false);
    send(
        cx,
        &a,
        Input::Cursor(CursorCommand::Move {
            direction: Direction::Forward,
            extend: false,
        }),
    );
    cx.update(|cx| assert!(a.read(cx).has_pending_input()));
    send(cx, &b, insert("X"));
    cx.update(|cx| {
        let view = a.read(cx);
        if let Some(cursor) = &view.editing.as_ref().unwrap().cursor {
            assert!(
                cursor
                    .snapshot()
                    .same_version(view.source.snapshot.as_ref().unwrap())
            );
        }
    });
    for _ in 0..32 {
        cx.executor().advance_clock(Duration::from_millis(2));
        cx.run_until_parked();
    }
    cx.update(|cx| {
        let view = a.read(cx);
        assert!(!view.has_pending_input());
        assert_eq!(view.selection.unwrap().head, text.len() as u64);
    });
}
