use super::*;

fn contents(snapshot: &TextSnapshot) -> Vec<u8> {
    snapshot.bytes(0..snapshot.len()).unwrap().to_vec()
}

fn assert_model(buffer: &EditBuffer, expected: &[u8]) {
    let snapshot = buffer.snapshot();
    piece_tree::check(&snapshot.root);
    assert_eq!(contents(&snapshot), expected);
    let mut starts = vec![0];
    starts.extend(
        expected
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i as u64 + 1),
    );
    assert_eq!(snapshot.line_count(), starts.len() as u64);
    for (line, &offset) in starts.iter().enumerate() {
        assert_eq!(snapshot.line_start(line as u64).unwrap(), Some(offset));
        assert_eq!(snapshot.line_at(offset).unwrap(), line as u64);
    }
    assert_eq!(snapshot.line_start(starts.len() as u64).unwrap(), None);
    for offset in (0..=expected.len()).step_by((expected.len() / 13).max(1)) {
        assert_eq!(
            snapshot.line_at(offset as u64).unwrap(),
            expected[..offset].iter().filter(|&&b| b == b'\n').count() as u64
        );
    }
}

fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn randomized_edits_undo_redo_and_snapshots_match_a_byte_vector() {
    for seed in 1..=32 {
        let mut random = seed;
        let mut buffer = EditBuffer::from_bytes("original\r\nα😀\n");
        buffer.set_history_budget(u64::MAX);
        let mut model = contents(&buffer.snapshot());
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut retained = Vec::new();
        for step in 0..1200 {
            match next(&mut random) % 8 {
                0 if !undo.is_empty() => {
                    redo.push(model.clone());
                    model = undo.pop().unwrap();
                    assert!(buffer.undo());
                }
                1 if !redo.is_empty() => {
                    undo.push(model.clone());
                    model = redo.pop().unwrap();
                    assert!(buffer.redo());
                }
                _ => {
                    let start = (next(&mut random) as usize) % (model.len() + 1);
                    let end = start + (next(&mut random) as usize) % (model.len() - start + 1);
                    let text = (0..next(&mut random) % 24)
                        .map(|_| match next(&mut random) % 7 {
                            0 => b'\n',
                            1 => b'\r',
                            2 => 0xff,
                            _ => next(&mut random) as u8,
                        })
                        .collect::<Vec<_>>();
                    if start != end || !text.is_empty() {
                        undo.push(model.clone());
                        redo.clear();
                    }
                    model.splice(start..end, text.iter().copied());
                    buffer
                        .replace(start as u64..end as u64, &text, UndoGroup::New)
                        .unwrap();
                }
            }
            assert_model(&buffer, &model);
            assert_eq!(buffer.can_undo(), !undo.is_empty());
            assert_eq!(buffer.can_redo(), !redo.is_empty());
            if step % 100 == 0 {
                retained.push((buffer.snapshot(), model.clone()));
            }
        }
        for (snapshot, expected) in retained {
            assert_eq!(contents(&snapshot), expected);
        }
    }
}

#[test]
fn sequential_and_middle_edits_keep_the_tree_balanced() {
    let mut buffer = EditBuffer::from_bytes(Bytes::new());
    buffer.set_history_budget(0);
    let mut expected = Vec::new();
    for i in 0..20_000 {
        let at = match i % 3 {
            0 => 0,
            1 => expected.len(),
            _ => expected.len() / 2,
        };
        let byte = b'a' + (i % 26) as u8;
        expected.insert(at, byte);
        buffer
            .replace(at as u64..at as u64, &[byte], UndoGroup::New)
            .unwrap();
        if i % 1000 == 0 {
            piece_tree::check(&buffer.current.root);
        }
    }
    assert_model(&buffer, &expected);
    assert!(buffer.current.root.as_ref().unwrap().height < 32);
    let old = buffer.snapshot();
    buffer.replace(1..19_999, b"\n", UndoGroup::New).unwrap();
    assert_model(&buffer, &[expected[0], b'\n', expected[19_999]]);
    assert_eq!(contents(&old), expected);
}

#[test]
fn grouped_typing_save_checkpoints_and_branches_preserve_history() {
    let mut buffer = EditBuffer::from_bytes("a");
    buffer.replace(1..1, b"b", UndoGroup::New).unwrap();
    buffer.replace(2..2, b"c", UndoGroup::Continue).unwrap();
    let saved = buffer.snapshot();
    assert!(buffer.mark_saved(&saved));
    assert!(!buffer.is_dirty());
    // A save closes the typing run without discarding it.
    buffer.replace(3..3, b"d", UndoGroup::Continue).unwrap();
    assert!(buffer.is_dirty());
    assert!(buffer.undo());
    assert_model(&buffer, b"abc");
    assert!(!buffer.is_dirty());
    assert!(buffer.undo());
    assert_model(&buffer, b"a");
    assert!(buffer.is_dirty());
    assert!(buffer.redo());
    assert!(!buffer.is_dirty());
    buffer.replace(3..3, b"e", UndoGroup::Continue).unwrap();
    assert!(!buffer.can_redo());
    assert_model(&buffer, b"abce");
    // Completion of an older asynchronous save must not clean newer edits.
    assert!(buffer.mark_saved(&saved));
    assert!(buffer.is_dirty());
    assert!(!buffer.mark_saved(&EditBuffer::from_bytes("abce").snapshot()));
    assert!(buffer.is_dirty());
}

#[test]
fn history_budget_evicts_old_transactions_without_losing_current_text() {
    let mut buffer = EditBuffer::from_bytes("original");
    for _ in 0..30 {
        buffer.replace(0..0, b"x", UndoGroup::New).unwrap();
    }
    let current = buffer.snapshot();
    buffer.set_history_budget(buffer.history_bytes() / 2);
    let mut undone = 0;
    while buffer.undo() {
        undone += 1;
    }
    assert!((1..30).contains(&undone));
    while buffer.redo() {}
    assert_eq!(contents(&buffer.snapshot()), contents(&current));
    assert!(buffer.undo());
    let retained = buffer.snapshot();
    buffer.set_history_budget(0);
    assert_eq!(buffer.history_bytes(), 0);
    assert!(!buffer.can_undo());
    assert!(!buffer.can_redo());
    assert_eq!(contents(&buffer.snapshot()), contents(&retained));
    assert!(buffer.is_dirty());
}

#[test]
fn failed_edits_and_empty_noops_preserve_revision_history_and_dirty_state() {
    let mut buffer = EditBuffer::from_bytes("abc");
    let before = buffer.snapshot();
    for range in [0..4, Range { start: 2, end: 1 }] {
        assert_eq!(
            buffer.replace(range, b"x", UndoGroup::New),
            Err(EditError::InvalidRange)
        );
    }
    buffer.replace(1..1, b"", UndoGroup::New).unwrap();
    assert_eq!(before.revision(), buffer.snapshot().revision());
    assert!(!buffer.is_dirty());
    assert!(!buffer.can_undo());
    assert_model(&buffer, b"abc");
}

#[test]
fn large_leaves_preserve_invalid_bytes_unicode_and_newlines_across_boundaries() {
    let mut expected = "é\r\n👩‍💻\n".repeat(20_000).into_bytes();
    expected.extend_from_slice(&[0xff, 0xc2, b'\n']);
    let mut buffer = EditBuffer::from_bytes(expected.clone());
    for range in [65_535..65_539, 131_071..131_076, 100..90_000] {
        expected.splice(range.clone(), [b'\n', 0xf0, b'\r', b'\n']);
        buffer
            .replace(
                range.start as u64..range.end as u64,
                &[b'\n', 0xf0, b'\r', b'\n'],
                UndoGroup::New,
            )
            .unwrap();
    }
    assert_model(&buffer, &expected);
    let snapshot = buffer.snapshot();
    let reader = std::thread::spawn(move || contents(&snapshot));
    buffer
        .replace(0..buffer.snapshot().len(), b"replacement", UndoGroup::New)
        .unwrap();
    assert_eq!(reader.join().unwrap(), expected);
}

fn paged_options() -> OpenOptions {
    OpenOptions {
        large_file_threshold: 0,
        block_cache: crate::BLOCK_SIZE * 4,
    }
}

#[test]
fn private_original_survives_external_overwrite_truncation_and_cache_eviction() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let bytes = (0..4 * 1024 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<_>>();
    fs::write(file.path(), &bytes).unwrap();
    let mut buffer = EditBuffer::open(
        file.path(),
        paged_options(),
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert!(buffer.is_chunked());
    let original = buffer.snapshot();
    // No source bytes need to be loaded to insert at the start.
    buffer.replace(0..0, b"prefix", UndoGroup::New).unwrap();
    fs::write(file.path(), b"externally replaced").unwrap();
    let edited = buffer.snapshot();
    for offset in [2_000_000, 1, 3_500_000, 700_000, 2_000_000] {
        let range = offset + 6..offset + 106;
        if let Err(ReadError::Missing(missing)) = edited.bytes(range.clone()) {
            assert_eq!(missing, range);
        }
        edited.load(range.clone()).unwrap();
        assert_eq!(
            &edited.bytes(range).unwrap()[..],
            &bytes[offset as usize..offset as usize + 100]
        );
        original.load(offset..offset + 100).unwrap();
        assert_eq!(
            &original.bytes(offset..offset + 100).unwrap()[..],
            &bytes[offset as usize..offset as usize + 100]
        );
    }
    assert_eq!(fs::read(file.path()).unwrap(), b"externally replaced");
}

#[test]
fn missing_edit_ranges_are_retryable_in_current_document_coordinates() {
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), vec![b'a'; 1024 * 1024]).unwrap();
    let mut buffer = EditBuffer::open(
        file.path(),
        paged_options(),
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    buffer.replace(0..0, b"prefix", UndoGroup::New).unwrap();
    let before = buffer.snapshot();
    let history = buffer.history_bytes();
    let range = 65_543..196_621;
    let mut misses = 0;
    loop {
        match buffer.replace(range.clone(), b"edited", UndoGroup::New) {
            Err(EditError::Read(ReadError::Missing(missing))) => {
                misses += 1;
                assert!(misses <= 3);
                assert_eq!(buffer.snapshot().revision(), before.revision());
                assert_eq!(buffer.history_bytes(), history);
                before.load(missing).unwrap();
            }
            Ok(()) => break,
            error => panic!("unexpected edit result: {error:?}"),
        }
    }
    assert!(misses > 0);
    assert_eq!(
        &buffer
            .snapshot()
            .bytes(range.start..range.start + 6)
            .unwrap()[..],
        b"edited"
    );
    assert!(buffer.undo());
    assert_eq!(buffer.snapshot().revision(), before.revision());
}

#[test]
fn preparation_is_cancellable_and_rejects_a_changed_original() {
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), vec![b'a'; 2 * 1024 * 1024]).unwrap();
    let cancelled = AtomicBool::new(false);
    let result = EditBuffer::open(file.path(), paged_options(), &cancelled, |copied, _| {
        if copied > 0 {
            cancelled.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
    let mut changed = false;
    let result = EditBuffer::open(
        file.path(),
        paged_options(),
        &AtomicBool::new(false),
        |copied, total| {
            if copied == total && !changed {
                changed = true;
                fs::write(file.path(), b"new file").unwrap();
            }
        },
    );
    assert!(result.err().unwrap().to_string().contains("changed"));
    assert_eq!(fs::read(file.path()).unwrap(), b"new file");
}

#[test]
fn preparation_rejects_atomic_path_replacement_even_when_length_is_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.txt");
    fs::write(&path, vec![b'a'; 65_536]).unwrap();
    let mut replacement = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    replacement.write_all(&vec![b'b'; 65_536]).unwrap();
    let mut replacement = Some(replacement);
    let result = EditBuffer::open(
        &path,
        paged_options(),
        &AtomicBool::new(false),
        |copied, total| {
            if copied == total {
                replacement.take().unwrap().persist(&path).unwrap();
            }
        },
    );
    assert!(result.err().unwrap().to_string().contains("changed"));
    assert_eq!(fs::read(&path).unwrap(), vec![b'b'; 65_536]);
}
