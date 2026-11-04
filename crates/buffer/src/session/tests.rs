use super::*;
use crate::{ByteSource, ReadError};

fn text(session: &DocumentSession) -> Vec<u8> {
    let snapshot = session.snapshot();
    snapshot
        .text
        .bytes(0..snapshot.text.len())
        .unwrap()
        .to_vec()
}

fn command(session: &mut DocumentSession, pane: PaneId, action: CursorCommand) {
    let base = session.pane_snapshot(pane).unwrap();
    let mut pending = session.cursor_command(&base, action).unwrap();
    loop {
        match pending.poll(2) {
            Ok(true) => break,
            Ok(false) => {}
            Err(SessionError::Text(TextError::Read(ReadError::Missing(range)))) => {
                pending.snapshot().load(range).unwrap()
            }
            error => panic!("{error:?}"),
        }
    }
    session.finish_command(&pending).unwrap();
}

#[test]
fn panes_share_text_and_history_but_keep_independent_rebased_selections() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("abcdef"));
    let left = session.attach_pane(Selection::caret(0)).unwrap();
    let right = session.attach_pane(Selection::new(4, 6)).unwrap();
    let before = session.snapshot();
    session
        .replace(
            &session.pane_snapshot(left).unwrap(),
            0..1,
            "AB",
            UndoGroup::New,
        )
        .unwrap();
    assert_eq!(text(&session), b"ABbcdef");
    assert_eq!(
        session.pane_snapshot(left).unwrap().selection(),
        Selection::caret(2)
    );
    assert_eq!(
        session.pane_snapshot(right).unwrap().selection(),
        Selection::new(5, 7)
    );
    assert_eq!(before.text.bytes(0..6).unwrap().as_ref(), b"abcdef");
    assert!(!session.is_current(&before));
    // A different pane cannot coalesce its input into the first pane's run.
    session
        .replace(
            &session.pane_snapshot(right).unwrap(),
            5..7,
            "!",
            UndoGroup::Continue,
        )
        .unwrap();
    assert_eq!(text(&session), b"ABbcd!");
    assert!(session.undo(&session.pane_snapshot(left).unwrap()).unwrap());
    assert_eq!(text(&session), b"ABbcdef");
    assert!(session.undo(&session.pane_snapshot(left).unwrap()).unwrap());
    assert_eq!(text(&session), b"abcdef");
    assert!(
        !session.is_current(&before),
        "undo must not revive an obsolete job"
    );
}

#[test]
fn grouped_unicode_typing_rebases_all_panes_through_undo_and_redo() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("tail"));
    let a = session.attach_pane(Selection::caret(0)).unwrap();
    let b = session.attach_pane(Selection::caret(0)).unwrap();
    session
        .replace(
            &session.pane_snapshot(a).unwrap(),
            0..0,
            "é",
            UndoGroup::New,
        )
        .unwrap();
    session
        .replace(
            &session.pane_snapshot(a).unwrap(),
            2..2,
            "😀",
            UndoGroup::Continue,
        )
        .unwrap();
    assert_eq!(
        session.pane_snapshot(b).unwrap().selection(),
        Selection::caret(6)
    );
    assert!(session.undo(&session.pane_snapshot(b).unwrap()).unwrap());
    assert_eq!(text(&session), b"tail");
    assert_eq!(
        session.pane_snapshot(a).unwrap().selection(),
        Selection::caret(0)
    );
    assert_eq!(
        session.pane_snapshot(b).unwrap().selection(),
        Selection::caret(0)
    );
    assert!(session.redo(&session.pane_snapshot(b).unwrap()).unwrap());
    assert_eq!(text(&session), "é😀tail".as_bytes());
    assert_eq!(
        session.pane_snapshot(a).unwrap().selection(),
        Selection::caret(6)
    );
    assert_eq!(
        session.pane_snapshot(b).unwrap().selection(),
        Selection::caret(6)
    );
}

#[test]
fn cursor_commands_move_select_and_delete_complete_graphemes() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("e\u{301}👩🏽‍💻\r\nz"));
    let pane = session.attach_pane(Selection::caret(0)).unwrap();
    command(
        &mut session,
        pane,
        CursorCommand::Move {
            direction: Direction::Forward,
            extend: false,
        },
    );
    assert_eq!(
        session.pane_snapshot(pane).unwrap().selection().head.offset,
        3
    );
    command(
        &mut session,
        pane,
        CursorCommand::Move {
            direction: Direction::Forward,
            extend: true,
        },
    );
    assert_eq!(
        session.pane_snapshot(pane).unwrap().selection().range(),
        3..18
    );
    // An unshifted arrow collapses a selection without skipping another cluster.
    command(
        &mut session,
        pane,
        CursorCommand::Move {
            direction: Direction::Backward,
            extend: false,
        },
    );
    assert_eq!(
        session.pane_snapshot(pane).unwrap().selection(),
        Selection::caret(3)
    );
    command(
        &mut session,
        pane,
        CursorCommand::Delete {
            direction: Direction::Forward,
        },
    );
    assert_eq!(text(&session), "e\u{301}\r\nz".as_bytes());
    command(
        &mut session,
        pane,
        CursorCommand::Delete {
            direction: Direction::Backward,
        },
    );
    assert_eq!(text(&session), b"\r\nz");
    command(
        &mut session,
        pane,
        CursorCommand::Delete {
            direction: Direction::Forward,
        },
    );
    assert_eq!(text(&session), b"z");
    assert!(session.undo(&session.pane_snapshot(pane).unwrap()).unwrap());
    assert_eq!(text(&session), b"\r\nz");
}

#[test]
fn stale_jobs_edits_and_commands_cannot_change_a_new_selection_or_document() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("a😀b"));
    let pane = session.attach_pane(Selection::caret(0)).unwrap();
    let base = session.pane_snapshot(pane).unwrap();
    let mut pending = session
        .cursor_command(
            &base,
            CursorCommand::Delete {
                direction: Direction::Forward,
            },
        )
        .unwrap();
    assert!(pending.poll(1).unwrap());
    session.set_selection(&base, Selection::caret(5)).unwrap();
    assert_eq!(session.finish_command(&pending), Err(SessionError::Stale));
    assert_eq!(
        session.replace(&base, 0..1, "x", UndoGroup::New),
        Err(SessionError::Stale)
    );
    assert_eq!(text(&session), "a😀b".as_bytes());
    let old = session.pane_snapshot(pane).unwrap();
    session.replace(&old, 0..1, "x", UndoGroup::New).unwrap();
    session.undo(&session.pane_snapshot(pane).unwrap()).unwrap();
    assert_eq!(
        session.replace(&old, 0..1, "y", UndoGroup::New),
        Err(SessionError::Stale)
    );
    let mut other = DocumentSession::new(EditBuffer::from_bytes("a😀b"));
    let other_pane = other.attach_pane(Selection::caret(0)).unwrap();
    assert_eq!(
        session.replace(
            &other.pane_snapshot(other_pane).unwrap(),
            0..1,
            "z",
            UndoGroup::New
        ),
        Err(SessionError::Stale)
    );
}

#[test]
fn closing_panes_cancels_their_requests_without_discarding_shared_content() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("abc"));
    let a = session.attach_pane(Selection::caret(0)).unwrap();
    let b = session.attach_pane(Selection::caret(3)).unwrap();
    let mut pending = session
        .cursor_command(
            &session.pane_snapshot(a).unwrap(),
            CursorCommand::Delete {
                direction: Direction::Forward,
            },
        )
        .unwrap();
    pending.poll(1).unwrap();
    assert!(session.detach_pane(a));
    let c = session.attach_pane(Selection::caret(0)).unwrap();
    assert_ne!(a, c);
    assert_eq!(
        session.finish_command(&pending),
        Err(SessionError::UnknownPane)
    );
    session
        .replace(
            &session.pane_snapshot(b).unwrap(),
            3..3,
            "!",
            UndoGroup::New,
        )
        .unwrap();
    assert_eq!(text(&session), b"abc!");
    assert_eq!(
        session.pane_snapshot(c).unwrap().selection(),
        Selection::caret(0)
    );
}

#[test]
fn save_completion_does_not_clean_later_edits_or_erase_undo() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("a"));
    let pane = session.attach_pane(Selection::caret(1)).unwrap();
    session
        .replace(
            &session.pane_snapshot(pane).unwrap(),
            1..1,
            "b",
            UndoGroup::New,
        )
        .unwrap();
    let saving = session.snapshot();
    session
        .replace(
            &session.pane_snapshot(pane).unwrap(),
            2..2,
            "c",
            UndoGroup::New,
        )
        .unwrap();
    assert!(session.mark_saved(&saving));
    assert!(session.is_dirty());
    assert!(session.can_undo());
    session.undo(&session.pane_snapshot(pane).unwrap()).unwrap();
    assert!(!session.is_dirty());
    session.set_history_budget(0);
    assert!(!session.can_undo());
    assert!(!session.can_redo());
    assert_eq!(text(&session), b"ab");
}

#[test]
fn anchors_resolve_to_scalar_edges_when_deletion_joins_damaged_utf8() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes(&b"\xf0xx\x9f\x98\x80"[..]));
    let a = session.attach_pane(Selection::caret(1)).unwrap();
    let b = session.attach_pane(Selection::new(1, 3)).unwrap();
    session
        .replace(&session.pane_snapshot(a).unwrap(), 1..3, "", UndoGroup::New)
        .unwrap();
    assert_eq!(text(&session), "😀".as_bytes());
    assert_eq!(session.pane_snapshot(a).unwrap().selection().head.offset, 4);
    assert_eq!(session.pane_snapshot(b).unwrap().selection().range(), 0..4);
}

#[test]
fn malformed_input_and_unready_commands_leave_the_session_unchanged() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("😀"));
    let pane = session.attach_pane(Selection::caret(0)).unwrap();
    let base = session.pane_snapshot(pane).unwrap();
    assert_eq!(
        session.replace(&base, 1..2, "x", UndoGroup::New),
        Err(SessionError::Text(TextError::InsideScalar))
    );
    assert_eq!(
        session.set_selection(&base, Selection::caret(1)),
        Err(SessionError::Text(TextError::InsideScalar))
    );
    let pending = session
        .cursor_command(
            &base,
            CursorCommand::Delete {
                direction: Direction::Forward,
            },
        )
        .unwrap();
    assert_eq!(session.finish_command(&pending), Err(SessionError::Pending));
    assert!(session.is_current(base.document()));
    assert!(!session.is_dirty());
    assert!(!session.can_undo());
}
