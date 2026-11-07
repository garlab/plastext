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
fn caret_scrolls_one_row_at_a_time_with_space_at_both_edges(cx: &mut TestAppContext) {
    let text: String = (0..100).map(|line| format!("line {line}\n")).collect();
    let (_dir, view, _other) = setup(cx, text.as_bytes(), false);
    cx.update(|cx| view.update(cx, |view, _| view.page_rows = 12));
    let top_line = |cx: &TestAppContext| {
        cx.update(|cx| {
            let view = view.read(cx);
            view.source.line_at(&view.index, view.top).unwrap().unwrap()
        })
    };
    for line in 1u64..=45 {
        send(cx, &view, Input::Vertical(1, false));
        assert_eq!(top_line(cx), line.saturating_sub(8));
    }
    let mut top = top_line(cx);
    for line in (0u64..45).rev() {
        send(cx, &view, Input::Vertical(-1, true));
        top = top.min(line.saturating_sub(3));
        assert_eq!(top_line(cx), top);
    }
    // Large jumps include the empty EOF row, and typing there follows it too.
    send(cx, &view, Input::DocumentEdge(true, false));
    assert_eq!(top_line(cx), 92);
    send(cx, &view, Input::Newline);
    assert_eq!(top_line(cx), 93);
    send(cx, &view, Input::DocumentEdge(false, false));
    assert_eq!(top_line(cx), 0);
}

#[gpui::test]
fn caret_scrolling_preserves_wheel_offsets_and_scales_to_short_panes(cx: &mut TestAppContext) {
    let text = "line\n".repeat(100);
    let (_dir, view, _other) = setup(cx, text.as_bytes(), false);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.page_rows = 12;
            view.top = 50;
            view.top_offset = px(7.);
        });
    });
    // Pointer selection at the bottom must not scroll text away from the mouse.
    send(cx, &view, Input::Selection(BufferSelection::caret(100)));
    cx.update(|cx| {
        assert_eq!(view.read(cx).top, 50);
        assert_eq!(view.read(cx).top_offset, px(7.));
    });
    send(cx, &view, Input::Selection(BufferSelection::caret(80)));
    send(cx, &view, Input::Vertical(-1, false));
    cx.update(|cx| {
        assert_eq!(view.read(cx).top, 50);
        assert_eq!(view.read(cx).top_offset, px(7.));
    });
    for rows in 1..=7 {
        cx.update(|cx| view.update(cx, |view, _| view.page_rows = rows));
        send(cx, &view, Input::DocumentEdge(true, false));
        cx.update(|cx| {
            let view = view.read(cx);
            let top = view.source.line_at(&view.index, view.top).unwrap().unwrap();
            assert!(top <= 100 && 100 - top < rows as u64);
            assert_eq!(view.top_offset, px(0.));
        });
        send(cx, &view, Input::DocumentEdge(false, false));
        cx.update(|cx| assert_eq!(view.read(cx).top, 0));
    }
}

#[gpui::test]
fn replacing_text_above_the_viewport_keeps_top_on_a_row_boundary(cx: &mut TestAppContext) {
    let (_dir, view, _other) = setup(cx, "line\n".repeat(100).as_bytes(), false);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.page_rows = 12;
            view.top = 50;
        });
    });
    send(cx, &view, Input::Selection(BufferSelection::new(0, 95)));
    send(cx, &view, insert(&"abcdef\n".repeat(14)));
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.top, 49);
        assert_eq!(view.source.row_start(view.top).unwrap(), view.top);
    });
}

#[gpui::test]
fn idle_caret_blinks_and_input_focus_and_read_mode_reset_it(cx: &mut TestAppContext) {
    let (_dir, view, other) = setup(cx, b"text\n", false);
    cx.update(crate::init);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            view.read(cx).focus_handle.focus(window);
            view.clone()
        })
        .unwrap()
    });
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    let state = |cx: &mut TestAppContext, handle: gpui::WindowHandle<TextView>| {
        // The test platform has no display link. Paint the focus change and its
        // follow-up invalidation, just as the next native frames would do.
        for _ in 0..2 {
            let window: gpui::AnyWindowHandle = handle.into();
            window
                .update(cx, |_, window, cx| window.draw(cx).clear())
                .unwrap();
            cx.run_until_parked();
        }
        handle
            .update(cx, |view, _, _| {
                let caret = &view.editing.as_ref().unwrap().caret;
                (caret.visible, caret.task.is_some())
            })
            .unwrap()
    };
    let tick = |cx: &mut TestAppContext, milliseconds| {
        cx.executor()
            .advance_clock(Duration::from_millis(milliseconds));
        cx.run_until_parked();
    };
    window
        .update(cx, |view, window, _| {
            assert!(window.is_window_active(), "window active");
            assert!(view.focus_handle.is_focused(window), "editor focused");
            assert!(view.last_hits.is_some(), "editor rendered");
        })
        .unwrap();
    assert_eq!(state(cx, window), (true, true));
    tick(cx, 499);
    assert_eq!(state(cx, window), (true, true));
    tick(cx, 1);
    assert_eq!(state(cx, window), (false, true));
    cx.simulate_keystrokes(window.into(), "right");
    assert_eq!(state(cx, window), (true, true));
    tick(cx, 500);
    assert_eq!(state(cx, window), (false, true));
    cx.simulate_keystrokes(window.into(), "x");
    assert_eq!(state(cx, window), (true, true));
    tick(cx, 500);
    assert_eq!(state(cx, window), (false, true));
    tick(cx, 500);
    assert_eq!(state(cx, window), (true, true));
    // Composition stays visible until it is committed or cancelled.
    window
        .update(cx, |view, window, cx| {
            view.replace_and_mark_text_in_range(None, "é", None, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    tick(cx, 1000);
    assert_eq!(state(cx, window), (true, false));
    window
        .update(cx, |view, window, cx| {
            view.unmark_text(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(state(cx, window), (true, true));
    // Neither an unfocused pane nor an inactive window keeps a blink timer.
    window.update(cx, |_, window, _| window.blur()).unwrap();
    cx.run_until_parked();
    tick(cx, 1000);
    assert_eq!(state(cx, window), (true, false));
    window
        .update(cx, |view, window, _| view.focus_handle.focus(window))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(state(cx, window), (true, true));
    let second = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            other.read(cx).focus_handle.focus(window);
            other.clone()
        })
        .unwrap()
    });
    second
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(state(cx, window), (true, false));
    assert_eq!(state(cx, second), (true, true));
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(state(cx, window), (true, true));
    assert_eq!(state(cx, second), (true, false));
    cx.simulate_keystrokes(window.into(), "cmd-z escape");
    assert_eq!(state(cx, window), (true, false));
    cx.update(|cx| assert!(!view.read(cx).is_editing()));
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    second
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

fn highlighted_rows(
    cx: &TestAppContext,
    view: &Entity<TextView>,
) -> Vec<(Range<u64>, plastext_syntax::Highlight)> {
    cx.update(|cx| {
        let view = view.read(cx);
        let (ranges, error) = view.source.visible_rows(&Folds::default(), 0, 100);
        assert_eq!(error, None);
        let rows: Vec<_> = ranges
            .into_iter()
            .map(|range| (range.start, view.source.bytes(range).unwrap(), true))
            .collect();
        view.highlights(&rows)
    })
}

#[gpui::test]
fn edit_mode_keeps_read_colors_and_refreshes_unsaved_syntax(cx: &mut TestAppContext) {
    for (name, original, changed) in [
        (
            "document.md",
            "# Heading\n\n```rust\nlet value = 1;\n```\n",
            "## New heading\n\n> quote\n",
        ),
        (
            "code.rs",
            "fn main() { let s = \"café\"; }\n",
            "// comment\nfn changed() { let n = 42; }\n",
        ),
        (
            "page.html",
            "<script>const n = 1;</script>\n",
            "<script type='application/json'>{\"n\":true}</script>\n",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, original).unwrap();
        let (source, index) = plastext_buffer::open(&path, OpenOptions::default()).unwrap();
        let language = plastext_syntax::detect(&path, original.as_bytes()).unwrap();
        let document =
            cx.update(|cx| EditingDocument::shared(path.clone(), OpenOptions::default(), cx));
        let view = cx.update(|cx| {
            cx.new(|cx| {
                let mut view = TextView::new(
                    source,
                    index,
                    TextStyle {
                        font_family: "Menlo".into(),
                        font_size: px(14.),
                        line_height: 1.4,
                        tab_size: 4,
                        sticky_scroll: false,
                        max_parse_size: 1024 * 1024,
                    },
                    Some(language),
                    cx,
                );
                view.set_editing_document(document.clone(), cx);
                view
            })
        });
        cx.run_until_parked();
        let before = highlighted_rows(cx, &view);
        assert!(!before.is_empty(), "{name}");
        // Publish the prepared original but do not run the editing parser yet:
        // the first edit-mode frame must retain the existing colors.
        let file = FileDocument::open(
            &path,
            OpenOptions::default(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        cx.update(|cx| {
            document.update(cx, |doc, cx| {
                doc.file = Some(file);
                doc.active = true;
                cx.notify();
            })
        });
        assert_eq!(highlighted_rows(cx, &view), before, "entry: {name}");
        cx.run_until_parked();
        assert_eq!(highlighted_rows(cx, &view), before, "prepared: {name}");
        send(
            cx,
            &view,
            Input::Selection(BufferSelection::new(0, original.len() as u64)),
        );
        send(cx, &view, insert(changed));
        // A second edit supersedes the first scheduled parse.
        send(cx, &view, insert("\n"));
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
        let current = content(cx, &view);
        let fresh =
            plastext_syntax::SyntaxTree::parse(language, current.as_bytes(), || false).unwrap();
        let expected: Vec<_> = fresh
            .highlights(current.as_bytes(), 0..current.len())
            .into_iter()
            .map(|(r, h)| (r.start as u64..r.end as u64, h))
            .collect();
        assert_eq!(highlighted_rows(cx, &view), expected, "edited: {name}");
        send(cx, &view, Input::Undo);
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
        assert_eq!(highlighted_rows(cx, &view), before, "undo: {name}");
    }
}

#[gpui::test]
fn edited_syntax_respects_size_limit_and_recovers_after_undo(cx: &mut TestAppContext) {
    let original = "# Heading\n";
    let (_dir, view, _other) = setup(cx, original.as_bytes(), false);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.language = plastext_syntax::detect(std::path::Path::new("a.md"), b"");
            view.style.max_parse_size = 64;
            view.refresh_edit_syntax(true, cx);
        });
    });
    cx.run_until_parked();
    let before = highlighted_rows(cx, &view);
    assert!(!before.is_empty());
    send(cx, &view, insert("another line\n"));
    // Cross the limit while that refresh is still pending.
    send(cx, &view, insert(&"x".repeat(128)));
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.syntax.is_none());
        assert!(view.syntax_text.is_none());
        assert!(view._edit_parse.is_none());
    });
    send(cx, &view, Input::Undo);
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();
    assert_eq!(content(cx, &view), original);
    assert_eq!(highlighted_rows(cx, &view), before);
}

#[gpui::test]
fn wrapped_cursor_selection_and_edits_follow_visual_rows(cx: &mut TestAppContext) {
    let original = b"one two three four five six\nlast\n";
    let (_dir, view, _other) = setup(cx, original, false);
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.word_wrap = true;
            view.wrapping.configure(Some(8));
            view.page_rows = 3;
        })
    });
    send(cx, &view, Input::Selection(BufferSelection::caret(1)));
    send(cx, &view, Input::Vertical(1, false));
    cx.update(|cx| assert_eq!(view.read(cx).selection.unwrap().head, 9));
    send(cx, &view, Input::Vertical(1, true));
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.apply_pending_jump(cx);
            let selected = view.selection.unwrap();
            assert_eq!((selected.anchor, selected.head), (9, 15));
            assert!(view.top > 0, "caret scrolls in visual rows");
            assert_eq!(view.scroll_x, px(0.));
        });
    });
    send(cx, &view, insert("replacement "));
    assert_eq!(
        content(cx, &view),
        "one two treplacement our five six\nlast\n"
    );
    send(cx, &view, Input::Undo);
    assert_eq!(content(cx, &view).as_bytes(), original);
    // A shorter soft row must not map its end back onto the following row.
    send(cx, &view, Input::Selection(BufferSelection::caret(7)));
    send(cx, &view, Input::Vertical(1, false));
    cx.update(|cx| assert_eq!(view.read(cx).selection.unwrap().head, 13));
    send(cx, &view, Input::Vertical(-1, false));
    cx.update(|cx| assert_eq!(view.read(cx).selection.unwrap().head, 7));
    cx.update(|cx| {
        view.update(cx, |view, _| {
            view.wrapping.configure(Some(40));
            assert_eq!(view.visual_row_start(15).unwrap(), 0);
            view.wrapping.configure(None);
            assert_eq!(view.visual_row_start(15).unwrap(), 0);
        })
    });
}

#[gpui::test]
fn returning_to_source_moves_the_old_caret_to_the_reading_position(cx: &mut TestAppContext) {
    let bytes = "line of text\n".repeat(100);
    let (_dir, view, _other) = setup(cx, bytes.as_bytes(), false);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.page_rows = 20;
            view.edit_at(13 * 80, cx);
            assert!(view.is_editing());
            assert_eq!(view.selection.unwrap().head, 13 * 80);
            assert!(view.top <= 13 * 80 && view.top >= 13 * 60);
        })
    });
    send(cx, &view, insert("here "));
    assert!(content(cx, &view)[13 * 80..].starts_with("here line of text"));
}

#[gpui::test]
fn requesting_source_during_file_operation_enters_editing_when_it_finishes(
    cx: &mut TestAppContext,
) {
    let (_dir, view, _other) = setup_with_ready(cx, b"original\n", false, false);
    let document = cx.update(|cx| view.read(cx).editing_document().unwrap());
    cx.update(|cx| {
        document.update(cx, |doc, cx| doc.set_file_operation_pending(true, cx));
        view.update(cx, |view, cx| view.edit_at(0, cx));
        assert!(!view.read(cx).is_editing());
        document.update(cx, |doc, cx| doc.set_file_operation_pending(false, cx));
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(view.read(cx).is_editing()));
    send(cx, &view, insert("changed "));
    assert_eq!(content(cx, &view), "changed original\n");
}

#[gpui::test]
fn deleting_and_undoing_keep_shifted_toml_colors_before_reparse(cx: &mut TestAppContext) {
    let text = "[workspace]\nmembers = [\"crates/app\"]\nresolver = \"2\"\n";
    let (_dir, view, other) = setup(cx, text.as_bytes(), false);
    for target in [&view, &other] {
        cx.update(|cx| {
            target.update(cx, |view, cx| {
                view.language = plastext_syntax::detect(std::path::Path::new("Cargo.toml"), b"");
                view.style.max_parse_size = 1024;
                view.refresh_edit_syntax(true, cx);
            })
        });
    }
    cx.run_until_parked();
    let before = highlighted_rows(cx, &view);
    let at = text.find("crates/app").unwrap() as u64 + 3;
    send(cx, &view, Input::Selection(BufferSelection::caret(at)));
    send(
        cx,
        &view,
        Input::Cursor(CursorCommand::Delete {
            direction: Direction::Backward,
        }),
    );
    // Do not advance the 40 ms parser timer: inspect the very next paint.
    for target in [&view, &other] {
        let spans = highlighted_rows(cx, target);
        for (range, kind) in before.iter().filter(|(r, _)| r.start >= at) {
            assert!(
                spans.contains(&(range.start - 1..range.end - 1, *kind)),
                "lost {range:?} {kind:?}"
            );
        }
    }
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();
    send(cx, &view, Input::Undo);
    let spans = highlighted_rows(cx, &view);
    for (range, kind) in before.iter().filter(|(r, _)| r.start >= at) {
        assert!(
            spans.contains(&(range.clone(), *kind)),
            "undo lost {range:?} {kind:?}"
        );
    }
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();
    assert_eq!(highlighted_rows(cx, &view), before);
}
