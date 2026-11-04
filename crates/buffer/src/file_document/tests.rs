use super::*;
use crate::{ByteSource, SavePhase, Selection, UndoGroup};
use std::fs;

fn cancel() -> AtomicBool {
    AtomicBool::new(false)
}
fn open(path: &Path) -> FileDocument {
    FileDocument::open(path, OpenOptions::default(), &cancel(), |_, _| {}).unwrap()
}
fn append(doc: &mut FileDocument, text: &str) {
    let session = doc.session_mut();
    let at = session.snapshot().text().len();
    let pane = session.attach_pane(Selection::caret(at)).unwrap();
    session
        .replace(
            &session.pane_snapshot(pane).unwrap(),
            at..at,
            text,
            UndoGroup::New,
        )
        .unwrap();
    session.detach_pane(pane);
}
fn text(doc: &FileDocument) -> Vec<u8> {
    let snapshot = doc.session().snapshot();
    snapshot
        .text()
        .bytes(0..snapshot.text().len())
        .unwrap()
        .to_vec()
}
fn check(doc: &mut FileDocument) -> ExternalResult {
    let observation = doc
        .external_check(OpenOptions::default())
        .run(&cancel(), |_, _| {});
    apply_external(doc, observation)
}

#[test]
fn saves_are_serialized_and_old_completion_does_not_clean_new_typing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    append(&mut doc, " first");
    let job = doc.begin_save(SaveOptions::default()).unwrap();
    assert!(doc.begin_save(SaveOptions::default()).is_none());
    let completion = job.run(&cancel(), |p| {
        if p.phase == SavePhase::Committing {
            append(&mut doc, " later");
        }
    });
    assert!(doc.is_saving());
    doc.finish_save(&completion).unwrap();
    assert!(!doc.is_saving());
    assert!(doc.session().is_dirty());
    assert_eq!(fs::read(&path).unwrap(), b"original first");
    assert_eq!(text(&doc), b"original first later");
    let pane = doc.session_mut().attach_pane(Selection::caret(0)).unwrap();
    let base = doc.session().pane_snapshot(pane).unwrap();
    doc.session_mut().undo(&base).unwrap();
    assert!(!doc.session().is_dirty());
    assert!(doc.session().can_undo());
}

#[test]
fn dropping_a_job_releases_the_save_slot_and_foreign_completions_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, b"a").unwrap();
    fs::write(&b, b"b").unwrap();
    let mut a = open(&a);
    let mut b = open(&b);
    drop(a.begin_save(SaveOptions::default()).unwrap());
    assert!(!a.is_saving());
    let job = a.begin_save(SaveOptions::default()).unwrap();
    let completion = job.run(&cancel(), |_| {});
    assert_eq!(
        b.finish_save(&completion).unwrap_err().kind,
        SaveFailureKind::Stale
    );
    assert!(a.is_saving());
    a.finish_save(&completion).unwrap();
    assert!(!a.is_saving());
    assert_eq!(text(&b), b"b");
}

#[test]
fn clean_reload_keeps_panes_clamps_anchors_and_returns_old_history_for_retirement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original text").unwrap();
    let mut doc = open(&path);
    let pane = doc
        .session_mut()
        .attach_pane(Selection::new(3, 13))
        .unwrap();
    let old = doc.session().snapshot();
    fs::write(&path, "é").unwrap();
    let result = check(&mut doc);
    assert_eq!(result.state, ExternalState::Reloaded);
    assert!(result.retired.is_some());
    assert!(!doc.session().is_dirty());
    assert!(!doc.session().is_current(&old));
    assert_eq!(
        doc.session().pane_snapshot(pane).unwrap().selection(),
        Selection::new(2, 2)
    );
    assert_eq!(old.text().bytes(0..13).unwrap().as_ref(), b"original text");
    assert_eq!(check(&mut doc).state, ExternalState::Unchanged);
}

#[test]
fn dirty_conflicts_preserve_edits_and_stale_reload_cannot_replace_new_typing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    fs::write(&path, b"outside").unwrap();
    let observation = doc
        .external_check(OpenOptions::default())
        .run(&cancel(), |_, _| {});
    append(&mut doc, " user edit");
    let result = apply_external(&mut doc, observation);
    assert_eq!(result.state, ExternalState::Conflict);
    assert!(result.retired.is_some());
    assert!(doc.has_conflict());
    assert_eq!(text(&doc), b"original user edit");
    assert_eq!(check(&mut doc).state, ExternalState::Conflict);
    let completion = doc
        .begin_save(SaveOptions::default())
        .unwrap()
        .run(&cancel(), |_| {});
    assert_eq!(
        doc.finish_save(&completion).unwrap_err().kind,
        SaveFailureKind::Conflict
    );
    assert!(doc.session().is_dirty());
    assert_eq!(fs::read(&path).unwrap(), b"outside");
}

#[test]
fn save_invalidates_older_watcher_results_and_ignores_its_own_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    append(&mut doc, " edited");
    let observation = doc
        .external_check(OpenOptions::default())
        .run(&cancel(), |_, _| {});
    let job = doc.begin_save(SaveOptions::default()).unwrap();
    assert_eq!(
        apply_external(&mut doc, observation).state,
        ExternalState::Deferred
    );
    let stale = doc
        .external_check(OpenOptions::default())
        .run(&cancel(), |_, _| {});
    doc.finish_save(&job.run(&cancel(), |_| {})).unwrap();
    assert_eq!(apply_external(&mut doc, stale).state, ExternalState::Stale);
    assert_eq!(check(&mut doc).state, ExternalState::Unchanged);
    assert!(!doc.has_conflict());
    assert!(!doc.session().is_dirty());
}

#[test]
fn deleted_file_keeps_contents_and_reappearing_file_reloads_only_when_clean() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    fs::remove_file(&path).unwrap();
    assert_eq!(check(&mut doc).state, ExternalState::Missing);
    assert_eq!(text(&doc), b"original");
    fs::write(&path, b"replacement").unwrap();
    assert_eq!(check(&mut doc).state, ExternalState::Reloaded);
    assert_eq!(text(&doc), b"replacement");
    assert!(!doc.has_conflict());
}

#[test]
fn edit_undo_cycle_cannot_revive_a_prepared_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    fs::write(&path, b"outside").unwrap();
    let observation = doc
        .external_check(OpenOptions::default())
        .run(&cancel(), |_, _| {});
    append(&mut doc, " edit");
    let pane = doc.session_mut().attach_pane(Selection::caret(0)).unwrap();
    let base = doc.session().pane_snapshot(pane).unwrap();
    doc.session_mut().undo(&base).unwrap();
    assert!(!doc.session().is_dirty());
    let result = apply_external(&mut doc, observation);
    assert_eq!(result.state, ExternalState::Stale);
    assert!(result.retired.is_some());
    assert_eq!(text(&doc), b"original");
}

#[test]
fn explicit_overwrite_requires_the_reviewed_disk_version_to_still_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"initial").unwrap();
    let mut doc = open(&path);
    append(&mut doc, " user edit");
    fs::write(&path, b"outside").unwrap();
    let reviewed = DiskVersion::read(&path).unwrap();
    fs::write(&path, b"a later outside edit").unwrap();
    let job = doc
        .begin_overwrite(reviewed, SaveOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        doc.finish_save(&job.run(&cancel(), |_| {}))
            .unwrap_err()
            .kind,
        SaveFailureKind::Conflict
    );
    assert_eq!(fs::read(&path).unwrap(), b"a later outside edit");
    let reviewed = DiskVersion::read(&path).unwrap();
    let job = doc
        .begin_overwrite(reviewed, SaveOptions::default())
        .unwrap()
        .unwrap();
    doc.finish_save(&job.run(&cancel(), |_| {})).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"initial user edit");
    assert!(!doc.session().is_dirty());
    assert!(!doc.has_conflict());
}

#[test]
fn explicit_discard_can_reload_unchanged_disk_but_never_discards_newer_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    append(&mut doc, " edit");
    let request = doc.reload_discarding_edits(OpenOptions::default());
    let result = apply_external(&mut doc, request.run(&cancel(), |_, _| {}));
    assert_eq!(result.state, ExternalState::Reloaded);
    assert!(result.retired.unwrap().is_dirty());
    assert_eq!(text(&doc), b"original");
    append(&mut doc, " edit");
    let request = doc.reload_discarding_edits(OpenOptions::default());
    append(&mut doc, " later");
    let result = apply_external(&mut doc, request.run(&cancel(), |_, _| {}));
    assert_eq!(result.state, ExternalState::Conflict);
    assert!(result.retired.is_some());
    assert_eq!(text(&doc), b"original edit later");
}

#[test]
fn older_unchanged_observation_cannot_clear_a_newer_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let mut doc = open(&path);
    append(&mut doc, " edit");
    let old = doc
        .external_check(OpenOptions::default())
        .run(&cancel(), |_, _| {});
    fs::write(&path, b"outside").unwrap();
    assert_eq!(check(&mut doc).state, ExternalState::Conflict);
    assert_eq!(apply_external(&mut doc, old).state, ExternalState::Stale);
    assert!(doc.has_conflict());
}

fn apply_external(doc: &mut FileDocument, mut observation: ExternalObservation) -> ExternalResult {
    doc.finish_external(&mut observation)
}

#[test]
fn reopen_encoding_is_explicit_clean_only_and_rejects_newer_input() {
    use crate::TextEncoding;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encoding.txt");
    fs::write(&path, b"caf\xe9").unwrap();
    let mut doc = FileDocument::open_with_encoding(
        &path,
        OpenOptions::default(),
        EncodingHint::Explicit(TextEncoding::UTF8),
        &cancel(),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(text(&doc), b"caf\xe9");
    let hint = EncodingHint::Explicit(TextEncoding::for_label("windows-1252").unwrap());
    let job = doc
        .reopen_with_encoding(hint, OpenOptions::default())
        .unwrap();
    assert_eq!(
        apply_external(&mut doc, job.run(&cancel(), |_, _| {})).state,
        ExternalState::Reloaded
    );
    assert_eq!(text(&doc), "café".as_bytes());
    let job = doc
        .reopen_with_encoding(
            EncodingHint::Explicit(TextEncoding::UTF8),
            OpenOptions::default(),
        )
        .unwrap();
    append(&mut doc, " edited");
    assert!(
        doc.reopen_with_encoding(hint, OpenOptions::default())
            .is_none()
    );
    assert_eq!(
        apply_external(&mut doc, job.run(&cancel(), |_, _| {})).state,
        ExternalState::Conflict
    );
    assert_eq!(text(&doc), "café edited".as_bytes());
}

#[test]
fn a_saved_format_is_the_checkpoint_and_the_encoding_for_later_reload() {
    use crate::{TextEncoding, TextFormat};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format.txt");
    fs::write(&path, b"hello").unwrap();
    let mut doc = open(&path);
    let latin = TextEncoding::for_label("windows-1252").unwrap();
    let format = TextFormat::new(latin, false).unwrap();
    let base = doc.session().snapshot();
    doc.session_mut().set_format(&base, format).unwrap();
    append(&mut doc, " café");
    let job = doc.begin_save(SaveOptions::default()).unwrap();
    let base = doc.session().snapshot();
    doc.session_mut()
        .set_format(&base, TextFormat::default())
        .unwrap();
    doc.finish_save(&job.run(&cancel(), |_| {})).unwrap();
    assert!(doc.session().is_dirty());
    let pane = doc.session_mut().attach_pane(Selection::caret(0)).unwrap();
    let base = doc.session().pane_snapshot(pane).unwrap();
    doc.session_mut().undo(&base).unwrap();
    assert!(!doc.session().is_dirty());
    fs::write(&path, b"Fran\xe7ais").unwrap();
    assert_eq!(check(&mut doc).state, ExternalState::Reloaded);
    assert_eq!(text(&doc), "Français".as_bytes());
    assert_eq!(doc.session().snapshot().text().format().encoding(), latin);
}
