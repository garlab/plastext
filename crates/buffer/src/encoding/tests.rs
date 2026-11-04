use super::*;
use crate::{
    DocumentSession, EditBuffer, ReadError, Selection, SessionError, TextSnapshot, UndoGroup,
};

fn text(snapshot: &TextSnapshot) -> Vec<u8> {
    snapshot.load(0..snapshot.len()).unwrap();
    snapshot.bytes(0..snapshot.len()).unwrap().to_vec()
}
fn saved(snapshot: &TextSnapshot) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    snapshot.write_to(&mut output, &AtomicBool::new(false), |_, _| {})?;
    Ok(output)
}
fn encoded(value: &str, encoding: TextEncoding, bom: bool) -> Vec<u8> {
    let mut result = if bom {
        encoding.bom().to_vec()
    } else {
        Vec::new()
    };
    if encoding.utf16() {
        for word in value.encode_utf16() {
            result.extend(if encoding == TextEncoding::UTF16_LE {
                word.to_le_bytes()
            } else {
                word.to_be_bytes()
            });
        }
    } else {
        let (bytes, _, errors) = encoding.codec().encode(value);
        assert!(!errors);
        result.extend_from_slice(&bytes);
    }
    result
}
fn open(raw: Vec<u8>, encoding: TextEncoding) -> EditBuffer {
    EditBuffer::from_encoded_bytes(raw, EncodingHint::Explicit(encoding)).unwrap()
}

#[test]
fn bom_detection_hides_only_the_initial_mark_and_roundtrips() {
    for encoding in [
        TextEncoding::UTF8,
        TextEncoding::UTF16_LE,
        TextEncoding::UTF16_BE,
    ] {
        for value in ["", "é 😀\r\nsecond\rthird\n\u{feff}body"] {
            let raw = encoded(value, encoding, true);
            let buffer = EditBuffer::from_encoded_bytes(raw.clone(), EncodingHint::Auto).unwrap();
            let snapshot = buffer.snapshot();
            assert_eq!(snapshot.format(), TextFormat::new(encoding, true).unwrap());
            assert_eq!(text(&snapshot), value.as_bytes());
            assert_eq!(saved(&snapshot).unwrap(), raw);
            assert!(!buffer.is_dirty());
            assert_eq!(snapshot.line_start(0).unwrap(), Some(0));
        }
    }
}

#[test]
fn default_utf8_heuristic_legacy_and_explicit_overrides() {
    assert_eq!(
        EditBuffer::from_encoded_bytes("ascii and 😀", EncodingHint::Auto)
            .unwrap()
            .snapshot()
            .format()
            .encoding(),
        TextEncoding::UTF8
    );
    let latin = TextEncoding::for_label("windows-1252").unwrap();
    let raw = b"Fran\xe7ais: le caf\xe9 est d\xe9j\xe0 pr\xeat. \xab Tr\xe8s bien ! \xbb";
    let buffer = EditBuffer::from_encoded_bytes(raw.as_slice(), EncodingHint::Auto).unwrap();
    assert_eq!(buffer.snapshot().format().encoding(), latin);
    assert_eq!(saved(&buffer.snapshot()).unwrap(), raw);
    let utf8 = open(raw.to_vec(), TextEncoding::UTF8);
    assert_eq!(text(&utf8.snapshot()), raw);
    assert_eq!(saved(&utf8.snapshot()).unwrap(), raw);
    assert!(TextEncoding::for_label("ISO-2022-JP").is_none());
    assert!(TextEncoding::for_label("replacement").is_none());
    assert!(TextEncoding::for_label("not-an-encoding").is_none());
    assert!(TextFormat::new(latin, true).is_err());
    assert!(
        EditBuffer::from_encoded_bytes(b"\xff\xfe\0\0".as_slice(), EncodingHint::Auto).is_err()
    );
    assert!(
        EditBuffer::from_encoded_bytes(
            b"\xef\xbb\xbftext".as_slice(),
            EncodingHint::Explicit(latin)
        )
        .is_err()
    );
}

#[test]
fn utf16_without_bom_and_malformed_units_preserve_original_bytes_after_edits() {
    for encoding in [TextEncoding::UTF16_LE, TextEncoding::UTF16_BE] {
        let mut raw = encoded("start ", encoding, false);
        for word in [0xd800u16, 0x20, 0xdc00] {
            raw.extend(if encoding == TextEncoding::UTF16_LE {
                word.to_le_bytes()
            } else {
                word.to_be_bytes()
            });
        }
        raw.extend(encoded(" end", encoding, false));
        raw.push(0xff);
        let mut buffer = open(raw.clone(), encoding);
        assert_eq!(text(&buffer.snapshot()), "start � � end�".as_bytes());
        assert_eq!(saved(&buffer.snapshot()).unwrap(), raw);
        buffer.replace_text(0..5, "edited", UndoGroup::New).unwrap();
        let mut expected = encoded("edited", encoding, false);
        expected.extend_from_slice(&raw[10..]);
        assert_eq!(saved(&buffer.snapshot()).unwrap(), expected);
        buffer.set_format(TextFormat::default());
        assert!(
            saved(&buffer.snapshot()).is_err(),
            "conversion must not discard damaged bytes"
        );
        buffer.undo();
        assert_eq!(saved(&buffer.snapshot()).unwrap(), expected);
    }
}

#[test]
fn edits_must_not_join_malformed_units_into_a_different_character() {
    let enc = TextEncoding::UTF16_LE;
    let mut buffer = open(vec![0x00, 0xd8, 0x20, 0, 0x00, 0xdc], enc);
    assert_eq!(text(&buffer.snapshot()), "� �".as_bytes());
    buffer.replace_text(3..4, "", UndoGroup::New).unwrap();
    assert!(
        saved(&buffer.snapshot()).is_err(),
        "joining high/low surrogates changes the text"
    );
    let mut odd = open(vec![0x61], enc);
    odd.replace_text(3..3, "x", UndoGroup::New).unwrap();
    assert!(
        saved(&odd.snapshot()).is_err(),
        "an odd byte must not shift the appended UTF-16"
    );
    let sjis = TextEncoding::for_label("shift_jis").unwrap();
    let mut buffer = open(vec![0x81, b' ', b'A'], sjis);
    assert_eq!(text(&buffer.snapshot()), "� A".as_bytes());
    buffer.replace_text(3..4, "", UndoGroup::New).unwrap();
    assert!(
        saved(&buffer.snapshot()).is_err(),
        "a former bad lead byte must not consume A"
    );
}

#[test]
fn legacy_aliases_and_multi_scalar_units_keep_their_original_spelling() {
    let sjis = TextEncoding::for_label("Shift_JIS").unwrap();
    let mut raw = b"before ".to_vec();
    raw.extend([0xfa, 0x40]); // Duplicate encoding of U+2170.
    raw.extend(b" after");
    let mut buffer = open(raw.clone(), sjis);
    assert_eq!(text(&buffer.snapshot()), "before ⅰ after".as_bytes());
    assert_eq!(saved(&buffer.snapshot()).unwrap(), raw);
    buffer.replace_text(0..6, "prefix", UndoGroup::New).unwrap();
    assert_eq!(&saved(&buffer.snapshot()).unwrap()[7..9], &[0xfa, 0x40]);
    let big5 = TextEncoding::for_label("Big5").unwrap();
    let mut buffer = open(vec![0x88, 0x62], big5); // U+00CA U+0304.
    assert_eq!(text(&buffer.snapshot()), "Ê̄".as_bytes());
    assert_eq!(saved(&buffer.snapshot()).unwrap(), &[0x88, 0x62]);
    buffer.replace_text(2..4, "", UndoGroup::New).unwrap();
    buffer.set_format(TextFormat::default());
    assert_eq!(saved(&buffer.snapshot()).unwrap(), "Ê".as_bytes());
}

#[test]
fn conversion_preserves_scalars_across_added_and_original_leaf_boundaries() {
    let mut value = "x".repeat(LEAF_BYTES - 1);
    value.push_str("😀é\r\nnext");
    for added in [false, true] {
        let mut buffer = if added {
            EditBuffer::from_bytes("")
        } else {
            EditBuffer::from_bytes(value.clone())
        };
        if added {
            buffer.replace_text(0..0, &value, UndoGroup::New).unwrap();
        }
        for encoding in [TextEncoding::UTF16_LE, TextEncoding::UTF16_BE] {
            buffer.set_format(TextFormat::new(encoding, true).unwrap());
            assert_eq!(
                saved(&buffer.snapshot()).unwrap(),
                encoded(&value, encoding, true)
            );
        }
    }
    let mut broken = EditBuffer::from_bytes(b"bad\xff".as_slice());
    broken.set_format(TextFormat::new(TextEncoding::UTF16_LE, false).unwrap());
    assert!(saved(&broken.snapshot()).is_err());
    let mut missing = EditBuffer::from_bytes("😀");
    missing.set_format(
        TextFormat::new(TextEncoding::for_label("windows-1252").unwrap(), false).unwrap(),
    );
    assert!(saved(&missing.snapshot()).is_err());
}

#[test]
fn newline_conversion_is_undoable_and_preserves_other_bytes() {
    let raw = b"a\r\nb\rc\n\xff\r";
    let mut buffer = EditBuffer::from_bytes(raw.as_slice());
    let original = buffer.snapshot();
    assert_eq!(
        original.line_endings(),
        crate::LineEndings {
            lf: 1,
            crlf: 1,
            cr: 2
        }
    );
    assert!(original.line_endings().is_mixed());
    assert_eq!(original.preferred_line_ending(), LineEnding::Cr);
    for ending in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
        buffer.set_format(TextFormat::default().with_line_ending(Some(ending)));
        let mut expected = Vec::new();
        for part in [b"a".as_slice(), b"b", b"c", b"\xff"] {
            expected.extend_from_slice(part);
            expected.extend_from_slice(ending.as_str().as_bytes());
        }
        assert_eq!(saved(&buffer.snapshot()).unwrap(), expected);
        assert_eq!(text(&buffer.snapshot()), raw);
        assert!(buffer.is_dirty());
        assert!(buffer.undo());
        assert!(!buffer.is_dirty());
        assert_eq!(saved(&buffer.snapshot()).unwrap(), raw);
    }
    assert_eq!(saved(&original).unwrap(), raw);
    // Conversion handles CR/LF in separate pieces, including a deleted separator.
    let mut pieces = EditBuffer::from_bytes("a\rx\nb");
    pieces.replace_text(2..3, "", UndoGroup::New).unwrap();
    pieces.set_format(TextFormat::default().with_line_ending(Some(LineEnding::Lf)));
    assert_eq!(saved(&pieces.snapshot()).unwrap(), b"a\nb");
    let mut utf16 = open(
        encoded("a\r\nb\rc\n", TextEncoding::UTF16_BE, true),
        TextEncoding::UTF16_BE,
    );
    utf16.set_format(
        utf16
            .snapshot()
            .format()
            .with_line_ending(Some(LineEnding::CrLf)),
    );
    assert_eq!(
        saved(&utf16.snapshot()).unwrap(),
        encoded("a\r\nb\r\nc\r\n", TextEncoding::UTF16_BE, true)
    );
}

#[test]
fn pane_settings_invalidate_old_commands_without_moving_selections() {
    let mut session = DocumentSession::new(EditBuffer::from_bytes("a\r\nb\r\n"));
    let a = session.attach_pane(Selection::caret(1)).unwrap();
    let b = session.attach_pane(Selection::new(3, 4)).unwrap();
    let base = session.snapshot();
    let stale = session.pane_snapshot(a).unwrap();
    let format = TextFormat::default().with_line_ending(Some(LineEnding::Cr));
    assert!(session.set_format(&base, format).unwrap());
    assert_eq!(
        session.pane_snapshot(b).unwrap().selection(),
        Selection::new(3, 4)
    );
    assert_eq!(session.insert_newline(&stale), Err(SessionError::Stale));
    session
        .insert_newline(&session.pane_snapshot(a).unwrap())
        .unwrap();
    assert_eq!(text(session.snapshot().text()), b"a\r\r\nb\r\n");
    session.undo(&session.pane_snapshot(a).unwrap()).unwrap();
    session.undo(&session.pane_snapshot(a).unwrap()).unwrap();
    assert!(!session.is_dirty());
    let pane = session.pane_snapshot(a).unwrap();
    session.insert_newline(&pane).unwrap();
    assert_eq!(text(session.snapshot().text()), b"a\r\n\r\nb\r\n");
}

#[test]
fn paged_projection_is_immutable_cancellable_and_does_not_fill_cache_when_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("utf16.txt");
    let value = "é😀\r\nline\r".repeat(80_000);
    let raw = encoded(&value, TextEncoding::UTF16_LE, true);
    std::fs::write(&path, &raw).unwrap();
    let options = OpenOptions {
        large_file_threshold: 0,
        block_cache: crate::BLOCK_SIZE * 4,
    };
    let cancel = AtomicBool::new(false);
    let mut buffer =
        EditBuffer::open_text(&path, options, EncodingHint::Auto, &cancel, |_, _| {}).unwrap();
    assert!(buffer.is_chunked());
    assert!(matches!(
        buffer.snapshot().bytes(0..1),
        Err(ReadError::Missing(_))
    ));
    std::fs::write(&path, b"rewritten elsewhere").unwrap();
    assert_eq!(saved(&buffer.snapshot()).unwrap(), raw);
    assert!(matches!(
        buffer.snapshot().bytes(0..1),
        Err(ReadError::Missing(_))
    ));
    buffer.snapshot().load(0..8).unwrap();
    buffer.replace_text(0..2, "prefix", UndoGroup::New).unwrap();
    let mut expected = encoded("prefix", TextEncoding::UTF16_LE, true);
    expected.extend_from_slice(&raw[4..]);
    assert_eq!(saved(&buffer.snapshot()).unwrap(), expected);
    std::fs::write(&path, raw).unwrap();
    let mut previous = 0;
    let result = EditBuffer::open_text(
        &path,
        options,
        EncodingHint::Auto,
        &cancel,
        |done, total| {
            assert!(done >= previous && done <= total);
            if done > total / 2 && done < total {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            previous = done;
        },
    );
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::Interrupted));
}

#[test]
fn random_legacy_and_utf16_originals_roundtrip_every_byte() {
    let mut seed = 17u64;
    for label in [
        "UTF-16LE",
        "UTF-16BE",
        "Shift_JIS",
        "Big5",
        "EUC-JP",
        "EUC-KR",
        "GB18030",
        "windows-1252",
        "windows-1251",
        "KOI8-R",
        "ISO-8859-2",
        "x-user-defined",
    ] {
        let encoding = TextEncoding::for_label(label).unwrap();
        for len in 0..96 {
            let bytes: Vec<_> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            let buffer = open(bytes.clone(), encoding);
            assert_eq!(saved(&buffer.snapshot()).unwrap(), bytes, "{label}, {len}");
        }
    }
}

#[test]
fn compatibility_mappings_must_not_silently_change_new_text() {
    let mut buffer = EditBuffer::from_bytes("¥");
    buffer
        .set_format(TextFormat::new(TextEncoding::for_label("Shift_JIS").unwrap(), false).unwrap());
    assert!(saved(&buffer.snapshot()).is_err());
    // The ambiguous UTF-32-looking prefix is a legitimate UTF-16 NUL when
    // explicitly requested, rather than guessed.
    let buffer = open(vec![0xff, 0xfe, 0, 0], TextEncoding::UTF16_LE);
    assert_eq!(text(&buffer.snapshot()), &[0]);
    assert_eq!(saved(&buffer.snapshot()).unwrap(), &[0xff, 0xfe, 0, 0]);
}
