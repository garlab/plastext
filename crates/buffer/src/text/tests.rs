use super::*;
use crate::unicode_summary::UnicodeSummary;

fn positions(bytes: &[u8]) -> Vec<(u64, u64)> {
    let mut result = vec![(0, 0)];
    let mut offset = 0;
    let mut utf16 = 0;
    for chunk in bytes.utf8_chunks() {
        for ch in chunk.valid().chars() {
            offset += ch.len_utf8() as u64;
            utf16 += ch.len_utf16() as u64;
            result.push((offset, utf16));
        }
        for _ in chunk.invalid() {
            offset += 1;
            utf16 += 1;
            result.push((offset, utf16));
        }
    }
    result
}

fn assert_coordinates(buffer: &EditBuffer, bytes: &[u8]) {
    let snapshot = buffer.snapshot();
    let expected = positions(bytes);
    assert_eq!(snapshot.scalar_count(), expected.len() as u64 - 1);
    assert_eq!(snapshot.utf16_len(), expected.last().unwrap().1);
    for (scalar, &(byte, units)) in expected.iter().enumerate() {
        assert_eq!(snapshot.byte_to_scalar(byte).unwrap(), scalar as u64);
        assert_eq!(snapshot.scalar_to_byte(scalar as u64).unwrap(), byte);
        assert_eq!(snapshot.byte_to_utf16(byte).unwrap(), units);
        assert_eq!(snapshot.utf16_to_byte(units, Bias::Before).unwrap(), byte);
        assert_eq!(snapshot.utf16_to_byte(units, Bias::After).unwrap(), byte);
        if let Some(&(next_byte, next_units)) = expected.get(scalar + 1) {
            for middle in byte + 1..next_byte {
                assert_eq!(snapshot.byte_to_utf16(middle), Err(TextError::InsideScalar));
            }
            if next_units - units == 2 {
                assert_eq!(
                    snapshot.utf16_to_byte(units + 1, Bias::Before).unwrap(),
                    byte
                );
                assert_eq!(
                    snapshot.utf16_to_byte(units + 1, Bias::After).unwrap(),
                    next_byte
                );
            }
        }
    }
    assert_eq!(
        snapshot.scalar_to_byte(snapshot.scalar_count() + 1),
        Err(TextError::OutOfBounds)
    );
    assert_eq!(
        snapshot.utf16_to_byte(snapshot.utf16_len() + 1, Bias::After),
        Err(TextError::OutOfBounds)
    );
}

#[test]
fn measures_are_associative_across_all_unicode_and_invalid_byte_splits() {
    for bytes in [
        "αé😀👩‍💻\r\n".as_bytes(),
        &b"\xf0\x9fxx\x98\x80\xe0\x80\x80\xff\xed\xa0\x80"[..],
        b"",
        b"ascii",
    ] {
        let whole = UnicodeSummary::new(bytes);
        for a in 0..=bytes.len() {
            assert_eq!(
                whole.split(bytes, a),
                (
                    UnicodeSummary::new(&bytes[..a]),
                    UnicodeSummary::new(&bytes[a..])
                )
            );
            for b in a..=bytes.len() {
                let left = UnicodeSummary::new(&bytes[..a]);
                let middle = UnicodeSummary::new(&bytes[a..b]);
                let right = UnicodeSummary::new(&bytes[b..]);
                assert_eq!(
                    left.join(a as u64, middle, (b - a) as u64).join(
                        b as u64,
                        right,
                        (bytes.len() - b) as u64
                    ),
                    whole
                );
                assert_eq!(
                    left.join(
                        a as u64,
                        middle.join((b - a) as u64, right, (bytes.len() - b) as u64),
                        (bytes.len() - a) as u64
                    ),
                    whole
                );
            }
        }
    }
}

#[test]
fn split_scalars_and_surrogates_have_exact_coordinates_after_raw_edits() {
    let mut buffer = EditBuffer::from_bytes("");
    let mut bytes = Vec::new();
    for byte in "é😀👩‍💻\r\n".bytes().chain([0xff, 0xc2, 0x80, 0xf0]) {
        buffer
            .replace(
                bytes.len() as u64..bytes.len() as u64,
                &[byte],
                UndoGroup::New,
            )
            .unwrap();
        bytes.push(byte);
        assert_coordinates(&buffer, &bytes);
    }
    let old = buffer.snapshot();
    for _ in 0..4 {
        buffer.replace(1..2, &[], UndoGroup::New).unwrap();
        bytes.remove(1);
        assert_coordinates(&buffer, &bytes);
    }
    assert_eq!(old.utf16_len(), 13);
    assert!(buffer.undo());
    let bytes = buffer.snapshot().bytes(0..buffer.snapshot().len()).unwrap();
    assert_coordinates(&buffer, &bytes);
}

#[test]
fn random_raw_edits_match_lossless_scalar_and_utf16_reference() {
    let mut seed = 7_u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut buffer = EditBuffer::from_bytes("start 😀\r\n");
    let mut bytes = b"start \xf0\x9f\x98\x80\r\n".to_vec();
    let alphabet = "😀🇫🇷क्षिé\r\n".as_bytes();
    for _ in 0..1200 {
        let start = random() as usize % (bytes.len() + 1);
        let end = start + random() as usize % (bytes.len() - start + 1);
        let inserted = (0..random() % 10)
            .map(|_| alphabet[random() as usize % alphabet.len()])
            .collect::<Vec<_>>();
        buffer
            .replace(start as u64..end as u64, &inserted, UndoGroup::New)
            .unwrap();
        bytes.splice(start..end, inserted);
        assert_coordinates(&buffer, &bytes);
    }
}

#[test]
fn coordinates_across_original_leaf_boundaries_and_lazy_cache_misses() {
    let mut bytes = vec![b'x'; 65_535];
    bytes.extend_from_slice("😀é".as_bytes());
    bytes.extend_from_slice(&vec![b'y'; 65_536]);
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &bytes).unwrap();
    let buffer = EditBuffer::open(
        file.path(),
        crate::OpenOptions {
            large_file_threshold: 0,
            block_cache: 1024 * 1024,
        },
        &std::sync::atomic::AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    let snapshot = buffer.snapshot();
    assert_eq!(snapshot.scalar_count(), 65_535 + 2 + 65_536);
    assert_eq!(snapshot.utf16_len(), snapshot.scalar_count() + 1);
    loop {
        match snapshot.byte_to_utf16(65_539) {
            Err(TextError::Read(ReadError::Missing(range))) => snapshot.load(range).unwrap(),
            Ok(units) => {
                assert_eq!(units, 65_537);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        snapshot.utf16_to_byte(65_536, Bias::Before).unwrap(),
        65_535
    );
    assert_eq!(snapshot.utf16_to_byte(65_536, Bias::After).unwrap(), 65_539);
}

#[test]
fn safe_text_edits_reject_partial_scalars_without_changing_history() {
    let mut buffer = EditBuffer::from_bytes("é😀");
    assert_eq!(
        buffer.replace_text(1..2, "x", UndoGroup::New),
        Err(TextError::InsideScalar)
    );
    assert!(!buffer.is_dirty());
    assert!(!buffer.can_undo());
    buffer.replace_text(2..6, "👩‍💻", UndoGroup::New).unwrap();
    assert_coordinates(&buffer, "é👩‍💻".as_bytes());
}
