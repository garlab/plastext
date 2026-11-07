use super::*;
use plastext_buffer::{
    DocumentSession, PaneSnapshot, Selection as BufferSelection, SessionError, TextEdit, TextError,
    UndoGroup,
};

pub(super) fn insert(
    session: &mut DocumentSession,
    base: &PaneSnapshot,
    input: &str,
    group: UndoGroup,
) -> Result<bool, SessionError> {
    let Some(prepared) = prepare(base, input, false)? else {
        return Ok(false);
    };
    session.apply_prepared(base, prepared, group)?;
    Ok(true)
}
pub(super) fn prepare(
    base: &PaneSnapshot,
    input: &str,
    loading: bool,
) -> Result<Option<plastext_buffer::PreparedEdits>, SessionError> {
    let close = match input {
        "(" => ")",
        "[" => "]",
        "{" => "}",
        "\"" => "\"",
        "'" => "'",
        "`" => "`",
        _ => "",
    };
    if close.is_empty() && !matches!(input, ")" | "]" | "}") {
        return Ok(None);
    }
    let text = base.document().text();
    let read = |range: Range<u64>| -> Result<Bytes, SessionError> {
        if loading {
            let mut bytes = Vec::new();
            text.read_uncached(range, &mut bytes)
                .map_err(|_| SessionError::Limit("Could not load pair input"))?;
            Ok(Bytes::from(bytes))
        } else {
            text.bytes(range)
                .map_err(|e| SessionError::Text(TextError::Read(e)))
        }
    };
    let mut edits = Vec::new();
    let mut payload = 0;

    let mut targets = Vec::new();
    let mut shift = 0i64;
    let primary = base.selection().range().start;
    let mut selections = base.selections();
    selections.sort_by_key(|s| s.range().start);
    let primary_index = selections
        .iter()
        .position(|s| s.range().start == primary)
        .unwrap_or(0);
    for selection in selections {
        let r = selection.range();
        if r.end - r.start > 65536 {
            return Err(SessionError::Limit(
                "Select at most 64 KiB to wrap with a pair",
            ));
        }
        let after = read(r.end..(r.end + 1).min(text.len()))?;
        let before = read(r.start.saturating_sub(1)..r.start)?;
        if r.is_empty()
            && after.as_ref() == input.as_bytes()
            && matches!(input, ")" | "]" | "}" | "\"" | "'" | "`")
        {
            targets.push(BufferSelection::caret(
                (r.end + 1).saturating_add_signed(shift),
            ));
            continue;
        }
        let pair = !close.is_empty()
            && (r.start != r.end
                || after
                    .first()
                    .is_none_or(|b| b.is_ascii_whitespace() || b")]}:,;".contains(b)))
            && (!matches!(input, "'" | "\"" | "`")
                || before
                    .last()
                    .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_' && *b != b'\\'));
        let inserted = if pair {
            let bytes = read(r.clone())?;
            let selected = std::str::from_utf8(&bytes)
                .map_err(|_| SessionError::Limit("Cannot wrap invalid text bytes"))?;
            format!("{input}{selected}{close}")
        } else {
            input.into()
        };
        let a = r.start.saturating_add_signed(shift) + input.len() as u64;
        let b = if pair { a + r.end - r.start } else { a };
        targets.push(if selection.anchor.offset > selection.head.offset {
            BufferSelection::new(b, a)
        } else {
            BufferSelection::new(a, b)
        });
        shift += inserted.len() as i64 - (r.end - r.start) as i64;
        payload += inserted.len();
        if payload > MAX_COPY as usize {
            return Err(SessionError::Limit("Pair insertion exceeds 64 MB"));
        }
        edits.push(TextEdit {
            range: r,
            text: inserted,
        });
    }
    targets.rotate_left(primary_index);
    let cancel = AtomicBool::new(false);
    let prepared = if loading {
        text.prepare_edits_loading(&edits, &cancel)?
            .with_selections(targets, &cancel)?
    } else {
        text.prepare_edits(&edits)?
            .with_cached_selections(targets)?
    };
    Ok(Some(prepared))
}
