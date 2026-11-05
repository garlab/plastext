use super::*;
use gpui::{TestAppContext, WindowHandle};
use plastext_buffer::OpenOptions;
use plastext_editor::EditingDocument;
use std::time::Duration;

fn open(cx: &mut TestAppContext) -> (tempfile::TempDir, WindowHandle<MarkdownView>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("document.md");
    std::fs::write(&path, "# Original\n\nBody\n").unwrap();
    let (source, index) = plastext_buffer::open(&path, OpenOptions::default()).unwrap();
    cx.update(|cx| {
        plastext_editor::init(cx);
        super::init(cx);
    });
    let window = cx.add_window(|window, cx| {
        let document = EditingDocument::shared(path.clone(), OpenOptions::default(), cx);
        let mut view = MarkdownView::new(
            path,
            source,
            index,
            TextStyle {
                font_family: "Menlo".into(),
                font_size: px(14.),
                line_height: 1.5,
                tab_size: 4,
                sticky_scroll: false,
                max_parse_size: 0,
            },
            Arc::default(),
            cx,
        );
        view.set_editing_document(document, cx);
        view.focus.focus(window);
        view
    });
    cx.run_until_parked();
    (dir, window)
}
fn paste(cx: &mut TestAppContext, window: WindowHandle<MarkdownView>, text: &str) {
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned())));
    cx.simulate_keystrokes(window.into(), "cmd-a cmd-v");
}
fn heading(cx: &mut TestAppContext, window: WindowHandle<MarkdownView>) -> String {
    window
        .update(cx, |view, _, _| view.document.headings[0].text.clone())
        .unwrap()
}

#[gpui::test]
fn markdown_source_enters_edit_mode_and_previews_unsaved_text(cx: &mut TestAppContext) {
    let (dir, window) = open(cx);
    cx.simulate_keystrokes(window.into(), "cmd-shift-v e");
    window
        .update(cx, |view, _, cx| {
            assert!(view.source_status(cx).unwrap().editing);
            assert_eq!(view.editing_document().unwrap().read(cx).view_count(), 1);
        })
        .unwrap();
    paste(
        cx,
        window,
        "# Unsaved heading\n\n[Read me][ref]\n\n[ref]: https://example.com\n",
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("document.md")).unwrap(),
        "# Original\n\nBody\n"
    );
    assert_eq!(
        heading(cx, window),
        "Original",
        "source-only typing does not reparse the preview"
    );
    cx.simulate_keystrokes(window.into(), "cmd-shift-v");
    assert_eq!(heading(cx, window), "Unsaved heading");
    window
        .update(cx, |view, _, cx| {
            assert!(view.rendered && view.preview_complete);
            assert!(view.editing_document().unwrap().read(cx).is_dirty());
            let blocks = view
                .document
                .parse_page(&view.preview_source, 0, || false)
                .unwrap();
            assert!(
                blocks
                    .iter()
                    .flat_map(|b| &b.spans)
                    .any(|span| span.style.link.as_deref() == Some("https://example.com"))
            );
        })
        .unwrap();
    // Saving works while the rendered preview has keyboard focus, too.
    cx.simulate_keystrokes(window.into(), "cmd-s");
    assert!(
        std::fs::read_to_string(dir.path().join("document.md"))
            .unwrap()
            .starts_with("# Unsaved heading")
    );
    window
        .update(cx, |view, _, cx| {
            assert!(!view.editing_document().unwrap().read(cx).is_dirty())
        })
        .unwrap();
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn markdown_source_double_click_enters_edit_mode(cx: &mut TestAppContext) {
    let (_dir, window) = open(cx);
    cx.simulate_keystrokes(window.into(), "cmd-shift-v");
    gpui::VisualTestContext::from_window(window.into(), cx).simulate_event(gpui::MouseDownEvent {
        position: gpui::point(px(100.), px(40.)),
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 2,
        first_mouse: false,
    });
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.source_status(cx).unwrap().editing)
        })
        .unwrap();
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn split_preview_debounces_edits_and_keeps_one_owner_per_tab(cx: &mut TestAppContext) {
    let (_dir, source_window) = open(cx);
    let preview = source_window
        .update(cx, |view, _, cx| view.duplicate(cx))
        .unwrap();
    let preview_window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, _| preview.clone())
            .unwrap()
    });
    cx.run_until_parked();
    cx.simulate_keystrokes(source_window.into(), "cmd-shift-v enter");
    paste(cx, source_window, "# First edit\n");
    paste(cx, source_window, "# Latest edit\n\nnew text\n");
    assert_eq!(heading(cx, preview_window), "Original");
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(heading(cx, preview_window), "Latest edit");
    let document = preview_window
        .update(cx, |view, _, cx| {
            let doc = view.editing_document().unwrap();
            assert_eq!(doc.read(cx).view_count(), 2);
            assert!(doc.read(cx).is_dirty());
            doc
        })
        .unwrap();
    source_window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(document.read(cx).view_count(), 1);
        assert!(document.read(cx).is_dirty());
    });
    preview_window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    drop(preview);
}
