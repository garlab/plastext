use super::*;
use crate::{EditBuffer, OpenOptions, ReadError, UndoGroup};
use unicode_segmentation::UnicodeSegmentation;

fn finish(search: &mut GraphemeSearch) -> (u64, usize) {
    let mut yields = 0;
    for _ in 0..20_000 {
        match search.poll(1) {
            Ok(Some(offset)) => return (offset, yields),
            Ok(None) => yields += 1,
            Err(TextError::Read(ReadError::Missing(range))) => {
                search.snapshot().load(range).unwrap()
            }
            error => panic!("unexpected cursor result: {error:?}"),
        }
    }
    panic!("cursor made no progress");
}

fn assert_navigation(snapshot: &TextSnapshot, text: &str) {
    let mut boundaries = text
        .grapheme_indices(true)
        .map(|(byte, _)| byte as u64)
        .collect::<Vec<_>>();
    boundaries.push(text.len() as u64);
    for (i, &at) in boundaries.iter().enumerate() {
        let mut forward = snapshot.grapheme_search(at, Direction::Forward).unwrap();
        let mut backward = snapshot.grapheme_search(at, Direction::Backward).unwrap();
        assert_eq!(
            finish(&mut forward).0,
            boundaries.get(i + 1).copied().unwrap_or(at)
        );
        assert_eq!(finish(&mut backward).0, boundaries[i.saturating_sub(1)]);
    }
}

#[test]
fn navigation_covers_crlf_combining_marks_emoji_flags_and_indic_clusters() {
    for text in ["", "a\r\nb", "e\u{301}é", "👩🏽‍💻👨‍👩‍👧‍👦", "🇫🇷🇬🇧🇯", "क्षि", "각"]
    {
        assert_navigation(&EditBuffer::from_bytes(text).snapshot(), text);
    }
    let snapshot = EditBuffer::from_bytes("e\u{301}👩🏽‍💻\r\n").snapshot();
    assert_eq!(
        finish(&mut snapshot.grapheme_search(0, Direction::Forward).unwrap()).0,
        3
    );
    assert_eq!(
        finish(&mut snapshot.grapheme_search(3, Direction::Forward).unwrap()).0,
        18
    );
    assert_eq!(
        finish(&mut snapshot.grapheme_search(20, Direction::Backward).unwrap()).0,
        18
    );
}

#[test]
fn storage_piece_and_chunk_boundaries_do_not_split_clusters() {
    let text = format!("{}e\u{301}👩🏽‍💻\r\n🇫🇷!", "x".repeat(65_535));
    let mut buffer = EditBuffer::from_bytes(text.clone());
    // Add a combining sequence one scalar at a time, each in its own piece.
    for ch in "e\u{301}👩🏽‍💻".chars().rev() {
        buffer
            .replace_text(0..0, &ch.to_string(), UndoGroup::New)
            .unwrap();
    }
    let snapshot = buffer.snapshot();
    let prefix = "e\u{301}👩🏽‍💻";
    assert_navigation(&EditBuffer::from_bytes(prefix).snapshot(), prefix);
    assert_eq!(
        finish(&mut snapshot.grapheme_search(3, Direction::Forward).unwrap()).0,
        prefix.len() as u64
    );
    let end_prefix = prefix.len() as u64 + 65_535;
    assert_eq!(
        finish(
            &mut snapshot
                .grapheme_search(end_prefix, Direction::Forward)
                .unwrap()
        )
        .0,
        end_prefix + 3
    );
    assert_eq!(
        finish(
            &mut snapshot
                .grapheme_search(end_prefix + 3, Direction::Backward)
                .unwrap()
        )
        .0,
        end_prefix
    );
    // Every chunk edge through a scalar/emoji sequence exercises context reads.
    for padding in 4088..4100 {
        let text = format!("{}👩🏽‍💻🇫🇷\r\n", "x".repeat(padding));
        let snapshot = EditBuffer::from_bytes(text.clone()).snapshot();
        for at in text.char_indices().skip(padding).map(|(i, _)| i) {
            let boundaries = text
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .chain([text.len()])
                .collect::<Vec<_>>();
            let next = boundaries
                .iter()
                .copied()
                .find(|&i| i > at)
                .unwrap_or(text.len());
            let previous = boundaries
                .iter()
                .copied()
                .rev()
                .find(|&i| i < at)
                .unwrap_or(0);
            assert_eq!(
                finish(
                    &mut snapshot
                        .grapheme_search(at as u64, Direction::Forward)
                        .unwrap()
                )
                .0,
                next as u64
            );
            assert_eq!(
                finish(
                    &mut snapshot
                        .grapheme_search(at as u64, Direction::Backward)
                        .unwrap()
                )
                .0,
                previous as u64
            );
        }
    }
}

#[test]
fn long_clusters_and_regional_context_yield_and_resume() {
    let text = format!("a{}z", "\u{301}".repeat(50_000));
    let snapshot = EditBuffer::from_bytes(text.clone()).snapshot();
    let mut forward = snapshot.grapheme_search(0, Direction::Forward).unwrap();
    assert_eq!(forward.poll(0).unwrap(), None);
    assert_eq!(forward.poll(1).unwrap(), None);
    let (end, yields) = finish(&mut forward);
    assert_eq!(end, text.len() as u64 - 1);
    assert!(yields > 10);
    assert_eq!(forward.poll(1).unwrap(), Some(end));
    let (start, yields) = finish(&mut snapshot.grapheme_search(end, Direction::Backward).unwrap());
    assert_eq!(start, 0);
    assert!(yields > 10);
    let flags = "🇫🇷".repeat(8000);
    let snapshot = EditBuffer::from_bytes(flags.clone()).snapshot();
    let (previous, yields) = finish(
        &mut snapshot
            .grapheme_search(flags.len() as u64, Direction::Backward)
            .unwrap(),
    );
    assert_eq!(previous, flags.len() as u64 - 8);
    assert!(yields > 1);
}

#[test]
fn malformed_bytes_remain_individual_units_and_scalar_interiors_are_rejected() {
    let snapshot = EditBuffer::from_bytes(&b"a\xff\xe0\x80\x80\xcc\x81b"[..]).snapshot();
    let mut at = 0;
    let mut boundaries = Vec::new();
    while at < snapshot.len() {
        at = finish(&mut snapshot.grapheme_search(at, Direction::Forward).unwrap()).0;
        boundaries.push(at);
    }
    assert_eq!(boundaries, [1, 2, 3, 4, 5, 7, 8]);
    let snapshot = EditBuffer::from_bytes("😀").snapshot();
    assert_eq!(
        snapshot
            .grapheme_search(1, Direction::Forward)
            .unwrap()
            .poll(1),
        Err(TextError::InsideScalar)
    );
}

#[test]
fn lazy_cursor_search_retains_its_version_through_eviction_and_edits() {
    let text = format!("a{}z", "\u{301}".repeat(600_000));
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &text).unwrap();
    let mut buffer = EditBuffer::open(
        file.path(),
        OpenOptions {
            large_file_threshold: 0,
            block_cache: 1024 * 1024,
        },
        &std::sync::atomic::AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    let snapshot = buffer.snapshot();
    let mut search = snapshot.grapheme_search(0, Direction::Forward).unwrap();
    assert!(matches!(
        search.poll(1),
        Err(TextError::Read(ReadError::Missing(_)))
    ));
    buffer.replace(0..0, b"new", UndoGroup::New).unwrap();
    let (end, yields) = finish(&mut search);
    assert_eq!(end, text.len() as u64 - 1);
    assert!(yields > 200);
    assert_eq!(snapshot.revision(), 0);
    assert_eq!(buffer.snapshot().revision(), 1);
}
