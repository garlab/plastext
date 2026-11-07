use super::*;
use gpui::TestAppContext;
use plastext_buffer::{Selection, UndoGroup};

fn setup(cx: &TestAppContext) -> (tempfile::TempDir, Entity<EditingDocument>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("document.txt");
    std::fs::write(&path, "original\n").unwrap();
    let file = FileDocument::open(
        &path,
        OpenOptions::default(),
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    let doc = cx.update(|cx| {
        let doc = EditingDocument::shared(path, OpenOptions::default(), cx);
        doc.update(cx, |doc, _| {
            doc.file = Some(file);
            doc.active = true;
        });
        doc
    });
    (dir, doc)
}
fn text(doc: &Entity<EditingDocument>, cx: &TestAppContext) -> String {
    cx.update(|cx| {
        let version = doc.read(cx).version().unwrap();
        String::from_utf8(
            version
                .text()
                .bytes(0..version.text().len())
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    })
}
fn append(doc: &Entity<EditingDocument>, value: &str, cx: &TestAppContext) {
    cx.update(|cx| {
        doc.update(cx, |doc, cx| {
            let session = doc.file.as_mut().unwrap().session_mut();
            let len = session.snapshot().text().len();
            let pane = session.attach_pane(Selection::caret(len)).unwrap();
            session
                .replace(
                    &session.pane_snapshot(pane).unwrap(),
                    len..len,
                    value,
                    UndoGroup::New,
                )
                .unwrap();
            session.detach_pane(pane);
            doc.changed(cx);
        })
    });
}
fn check(doc: &Entity<EditingDocument>, cx: &mut TestAppContext) {
    cx.update(|cx| doc.update(cx, |doc, cx| doc.external_changed(cx)));
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(350));
    cx.run_until_parked();
}

#[gpui::test]
fn clean_changes_reload_and_deletion_keeps_the_snapshot(cx: &mut TestAppContext) {
    let (dir, doc) = setup(cx);
    let path = dir.path().join("document.txt");
    cx.update(|cx| assert!(doc.read(cx).can_close_as_saved()));
    std::fs::write(&path, "updated\n").unwrap();
    check(&doc, cx);
    assert_eq!(text(&doc, cx), "updated\n");
    std::fs::remove_file(&path).unwrap();
    check(&doc, cx);
    assert_eq!(text(&doc, cx), "updated\n");
    cx.update(|cx| {
        assert_eq!(doc.read(cx).disk_change(), Some(DiskChange::Missing));
        assert!(!doc.read(cx).can_close_as_saved());
        doc.update(cx, |doc, cx| doc.keep_mine(cx));
        assert_eq!(doc.read(cx).disk_change(), None);
        assert!(!doc.read(cx).can_close_as_saved());
    });
    std::fs::write(&path, "returned\n").unwrap();
    check(&doc, cx);
    assert_eq!(text(&doc, cx), "returned\n");
    cx.update(|cx| {
        assert_eq!(doc.read(cx).disk_change(), None);
        assert!(doc.read(cx).can_close_as_saved());
    });
}

#[gpui::test]
fn keep_mine_requires_confirmation_and_overwrite_rejects_a_newer_disk_version(
    cx: &mut TestAppContext,
) {
    let (dir, doc) = setup(cx);
    let path = dir.path().join("document.txt");
    append(&doc, "mine", cx);
    std::fs::write(&path, "outside").unwrap();
    check(&doc, cx);
    assert_eq!(text(&doc, cx), "original\nmine");
    let reviewed = cx.update(|cx| {
        assert_eq!(doc.read(cx).disk_change(), Some(DiskChange::Changed));
        doc.update(cx, |doc, cx| {
            doc.keep_mine(cx);
            doc.save(cx);
        });
        assert!(doc.read(cx).confirming_overwrite());
        doc.read(cx).reviewed_disk().unwrap()
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "outside");
    std::fs::write(&path, "newer outside").unwrap();
    cx.update(|cx| doc.update(cx, |doc, cx| doc.overwrite_disk(reviewed, cx)));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer outside");
    check(&doc, cx);
    let reviewed = cx.update(|cx| doc.read(cx).reviewed_disk().unwrap());
    cx.update(|cx| doc.update(cx, |doc, cx| doc.overwrite_disk(reviewed, cx)));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\nmine");
    check(&doc, cx);
    cx.update(|cx| {
        assert!(!doc.read(cx).is_dirty());
        assert_eq!(doc.read(cx).disk_change(), None);
    });
}

#[gpui::test]
fn typing_during_explicit_reload_is_never_discarded(cx: &mut TestAppContext) {
    let (dir, doc) = setup(cx);
    append(&doc, "mine", cx);
    std::fs::write(dir.path().join("document.txt"), "outside").unwrap();
    check(&doc, cx);
    cx.update(|cx| doc.update(cx, |doc, cx| doc.reload_from_disk(cx)));
    append(&doc, " newer", cx);
    cx.run_until_parked();
    assert_eq!(text(&doc, cx), "original\nmine newer");
    check(&doc, cx);
    cx.update(|cx| doc.update(cx, |doc, cx| doc.reload_from_disk(cx)));
    cx.run_until_parked();
    assert_eq!(text(&doc, cx), "outside");
    cx.update(|cx| assert!(!doc.read(cx).is_dirty()));
}
