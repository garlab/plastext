use super::*;
use plastext_buffer::{
    LineEnding, PreparedEdits, TextEdit, TextEncoding, TextFormat, TextSnapshot,
};

pub(super) fn format(
    text: &TextSnapshot,
    options: &plastext_settings::Editor,
) -> std::io::Result<TextFormat> {
    if options.charset.as_deref() == Some("latin1") {
        return Err(std::io::Error::other(
            "ISO-8859-1 output is not supported; choose UTF-8 or Windows-1252 explicitly",
        ));
    }
    let current = text.format();
    let output = if let Some(label) = options.charset.as_deref() {
        let (label, bom) = match label {
            "utf-8-bom" => ("utf-8", true),
            "utf-16le" | "utf-16be" => (label, true),
            other => (other, false),
        };
        let encoding = TextEncoding::for_label(label)
            .ok_or_else(|| std::io::Error::other("Unsupported output encoding"))?;
        TextFormat::new(encoding, bom)?.with_line_ending(current.line_ending())
    } else {
        current
    };
    Ok(if let Some(ending) = options.end_of_line.as_deref() {
        output.with_line_ending(Some(match ending {
            "lf" => LineEnding::Lf,
            "crlf" => LineEnding::CrLf,
            "cr" => LineEnding::Cr,
            _ => return Err(std::io::Error::other("Invalid end_of_line setting")),
        }))
    } else {
        output
    })
}

pub(super) fn prepare(
    text: &TextSnapshot,
    options: &plastext_settings::Editor,
    cancel: &AtomicBool,
) -> Result<(PreparedEdits, TextFormat), String> {
    let output = format(text, options).map_err(|e| e.to_string())?;
    let mut edits = Vec::new();
    let mut whitespace = None;
    let mut ending = None;
    if options.trim_trailing_whitespace || options.insert_final_newline.is_some() {
        let mut at = 0;
        let mut bytes = Vec::new();
        while at < text.len() {
            if cancel.load(Ordering::Relaxed) {
                return Err("Save preparation cancelled".into());
            }
            let end = (at + 256 * 1024).min(text.len());
            bytes.clear();
            text.read_uncached(at..end, &mut bytes)
                .map_err(|e| e.to_string())?;
            for (i, b) in bytes.iter().enumerate() {
                let offset = at + i as u64;
                if matches!(b, b'\r' | b'\n') {
                    if let Some(start) = whitespace.take()
                        && options.trim_trailing_whitespace
                    {
                        edits.push(TextEdit {
                            range: start..offset,
                            text: String::new(),
                        });
                    }
                    ending.get_or_insert(offset);
                } else {
                    ending = None;
                    if matches!(b, b' ' | b'\t') {
                        whitespace.get_or_insert(offset);
                    } else {
                        whitespace = None;
                    }
                }
                if edits.len() > 100_000 {
                    return Err("Save cleanup exceeds 100,000 changes; disable trim_trailing_whitespace or use a smaller file".into());
                }
            }
            at = end;
        }
        let mut end = text.len();
        if options.trim_trailing_whitespace
            && let Some(start) = whitespace
        {
            edits.push(TextEdit {
                range: start..end,
                text: String::new(),
            });
            end = start;
            // Determine whether trimming exposes an existing line terminator.
            if end > 0 {
                let mut b = Vec::new();
                text.read_uncached(end - 1..end, &mut b)
                    .map_err(|e| e.to_string())?;
                if matches!(b[0], b'\r' | b'\n') {
                    ending = Some(end - 1);
                }
            }
        }
        match options.insert_final_newline {
            Some(true) if end > 0 && ending.is_none() => edits.push(TextEdit {
                range: end..end,
                text: output
                    .line_ending()
                    .unwrap_or(text.preferred_line_ending())
                    .as_str()
                    .into(),
            }),
            Some(false) => {} // false disables insertion; existing endings remain intact.
            _ => {}
        }
    }
    let prepared = text
        .prepare_edits_loading(&edits, cancel)
        .map_err(|e| format!("Could not prepare save: {e:?}"))?;
    Ok((prepared, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_buffer::{DocumentSession, EditBuffer, Selection, UndoGroup};
    #[test]
    fn save_cleanup_is_undoable_and_format_is_explicit() {
        let original = "α  \r\nb\t  ";
        let mut session = DocumentSession::new(EditBuffer::from_bytes(original));
        let pane = session.attach_pane(Selection::caret(0)).unwrap();
        let base = session.pane_snapshot(pane).unwrap();
        let options = plastext_settings::Editor {
            trim_trailing_whitespace: true,
            insert_final_newline: Some(true),
            charset: Some("utf-16le".into()),
            end_of_line: Some("lf".into()),
            ..Default::default()
        };
        let (prepared, format) =
            prepare(base.document().text(), &options, &AtomicBool::new(false)).unwrap();
        assert_eq!(format.encoding(), TextEncoding::UTF16_LE);
        assert!(format.has_bom());
        assert_eq!(format.line_ending(), Some(LineEnding::Lf));
        session
            .apply_prepared(&base, prepared, UndoGroup::New)
            .unwrap();
        let text = session.snapshot();
        assert_eq!(
            text.text().bytes(0..text.text().len()).unwrap().as_ref(),
            "α\r\nb\n".as_bytes()
        );
        session.undo(&session.pane_snapshot(pane).unwrap()).unwrap();
        let text = session.snapshot();
        assert_eq!(
            text.text().bytes(0..text.text().len()).unwrap().as_ref(),
            original.as_bytes()
        );
    }
}
