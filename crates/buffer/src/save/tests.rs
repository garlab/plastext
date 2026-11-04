use super::*;
use crate::{ByteSource, EditBuffer, OpenOptions, ReadError, UndoGroup};
use std::sync::atomic::Ordering;

fn cancelled() -> AtomicBool {
    AtomicBool::new(false)
}
fn edit(bytes: impl Into<bytes::Bytes>) -> TextSnapshot {
    EditBuffer::from_bytes(bytes).snapshot()
}
fn entries(path: &Path) -> usize {
    fs::read_dir(path).unwrap().count()
}

#[test]
fn streaming_roundtrips_raw_bytes_and_does_not_populate_the_read_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("raw.bin");
    let mut expected: Vec<_> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    fs::write(&path, &expected).unwrap();
    let mut buffer = EditBuffer::open(
        &path,
        OpenOptions {
            large_file_threshold: 0,
            block_cache: 1024 * 1024,
        },
        &cancelled(),
        |_, _| {},
    )
    .unwrap();
    buffer.snapshot().load(2_000_000..2_100_000).unwrap();
    buffer
        .replace(2_000_010..2_000_019, b"\xff\r\nnew", UndoGroup::New)
        .unwrap();
    expected.splice(2_000_010..2_000_019, b"\xff\r\nnew".iter().copied());
    let snapshot = buffer.snapshot();
    assert!(matches!(snapshot.bytes(0..1), Err(ReadError::Missing(_))));
    struct Sink {
        bytes: Vec<u8>,
        largest: usize,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.largest = self.largest.max(bytes.len());
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink {
        bytes: Vec::new(),
        largest: 0,
    };
    let mut previous = 0;
    snapshot
        .write_to(&mut sink, &cancelled(), |done, total| {
            assert!(done >= previous && done <= total);
            previous = done;
        })
        .unwrap();
    assert_eq!(sink.bytes, expected);
    assert_eq!(previous, snapshot.len());
    assert!(sink.largest <= 64 * 1024);
    assert!(matches!(snapshot.bytes(0..1), Err(ReadError::Missing(_))));
}

#[test]
fn atomic_save_preserves_bytes_and_returns_a_new_disk_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("文書.txt");
    fs::write(&path, b"old").unwrap();
    let version = DiskVersion::read(&path).unwrap();
    for data in [b"\xffnew\r\n\0\xef\xbb\xbf".as_slice(), b"", b"last"] {
        let version = DiskVersion::read(&path).unwrap();
        let receipt = save_snapshot(
            &edit(data),
            &version,
            SaveOptions::default(),
            &cancelled(),
            |_| {},
        )
        .unwrap();
        assert!(!receipt.in_place);
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(receipt.version.is_current().unwrap());
        assert!(!version.is_current().unwrap());
        assert_eq!(entries(dir.path()), 1);
    }
    assert!(!version.is_current().unwrap());
}

#[test]
fn cancellation_at_every_preparation_phase_leaves_the_destination_unchanged() {
    for atomic in [true, false] {
        for phase in [
            SavePhase::Writing,
            SavePhase::BackingUp,
            SavePhase::Committing,
        ] {
            if atomic && phase == SavePhase::BackingUp {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("cancel.txt");
            let original = vec![b'o'; 200_000];
            fs::write(&path, &original).unwrap();
            let version = DiskVersion::read(&path).unwrap();
            let cancel = cancelled();
            let result = save_snapshot(
                &edit(vec![b'n'; 250_000]),
                &version,
                SaveOptions { atomic },
                &cancel,
                |p| {
                    if p.phase == phase && (phase == SavePhase::Committing || p.completed >= 65_536)
                    {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
            )
            .unwrap_err();
            assert_eq!(result.kind, SaveFailureKind::Cancelled);
            assert!(!result.target_may_have_changed);
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(entries(dir.path()), 1);
        }
    }
}

#[test]
fn external_changes_during_staging_or_at_commit_are_not_overwritten() {
    for phase in [SavePhase::Writing, SavePhase::Committing] {
        for atomic in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("conflict.txt");
            fs::write(&path, b"initial").unwrap();
            let version = DiskVersion::read(&path).unwrap();
            let mut rewritten = false;
            let failure = save_snapshot(
                &edit("ours"),
                &version,
                SaveOptions { atomic },
                &cancelled(),
                |p| {
                    if p.phase == phase && !rewritten {
                        let replacement = dir.path().join("replacement");
                        fs::write(&replacement, b"outside writer").unwrap();
                        fs::rename(replacement, &path).unwrap();
                        rewritten = true;
                    }
                },
            )
            .unwrap_err();
            assert_eq!(failure.kind, SaveFailureKind::Conflict);
            assert!(!failure.target_may_have_changed);
            assert_eq!(fs::read(&path).unwrap(), b"outside writer");
            assert_eq!(entries(dir.path()), 1);
        }
    }
}

#[test]
fn replacement_deletion_and_same_length_restored_mtime_are_detected() {
    for change in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"old").unwrap();
        let before = DiskVersion::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        match change {
            0 => {
                fs::remove_file(&path).unwrap();
                fs::write(&path, b"new").unwrap();
            }
            1 => {
                fs::remove_file(&path).unwrap();
            }
            _ => {
                let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.write_all(b"new").unwrap();
                file.set_times(fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
        }
        assert!(!before.is_current().unwrap());
        assert_eq!(
            save_snapshot(
                &edit("ours"),
                &before,
                SaveOptions::default(),
                &cancelled(),
                |_| {}
            )
            .unwrap_err()
            .kind,
            SaveFailureKind::Conflict
        );
        if change != 1 {
            assert_eq!(fs::read(&path).unwrap(), b"new");
        }
    }
}

#[test]
fn hard_links_and_explicit_in_place_saves_preserve_the_inode() {
    for linked in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        let alias = dir.path().join("alias");
        fs::write(&path, b"a longer original").unwrap();
        if linked {
            fs::hard_link(&path, &alias).unwrap();
        }
        let before = DiskVersion::read(&path).unwrap();
        let receipt = save_snapshot(
            &edit("short"),
            &before,
            SaveOptions { atomic: linked },
            &cancelled(),
            |_| {},
        )
        .unwrap();
        assert!(receipt.in_place);
        assert_eq!(fs::read(&path).unwrap(), b"short");
        if linked {
            assert_eq!(fs::read(alias).unwrap(), b"short");
        }
        assert_eq!(entries(dir.path()), if linked { 2 } else { 1 });
    }
}

#[test]
fn in_place_commit_finishes_if_cancellation_arrives_after_writing_starts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"old").unwrap();
    let cancel = cancelled();
    let expected = vec![b'n'; 200_000];
    save_snapshot(
        &edit(expected.clone()),
        &DiskVersion::read(&path).unwrap(),
        SaveOptions { atomic: false },
        &cancel,
        |p| {
            if p.phase == SavePhase::Committing && p.completed > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(fs::read(&path).unwrap(), expected);
    assert_eq!(entries(dir.path()), 1);
}

#[test]
fn uncertain_in_place_commit_retains_the_pre_save_bytes_for_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original recovery content").unwrap();
    let mut moved = false;
    let error = save_snapshot(
        &edit(vec![b'n'; 200_000]),
        &DiskVersion::read(&path).unwrap(),
        SaveOptions { atomic: false },
        &cancelled(),
        |p| {
            if p.phase == SavePhase::Committing && p.completed > 0 && !moved {
                fs::rename(&path, dir.path().join("moved")).unwrap();
                fs::write(&path, b"outside replacement").unwrap();
                moved = true;
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, SaveFailureKind::Conflict);
    assert!(error.target_may_have_changed);
    assert_eq!(
        fs::read(error.recovery_path.unwrap()).unwrap(),
        b"original recovery content"
    );
    assert_eq!(fs::read(path).unwrap(), b"outside replacement");
}

#[cfg(unix)]
#[test]
fn symlinks_write_through_and_retargeting_before_commit_aborts() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let other = dir.path().join("other");
    let link = dir.path().join("link");
    fs::write(&target, b"old").unwrap();
    fs::write(&other, b"other").unwrap();
    symlink("target", &link).unwrap();
    let expected = DiskVersion::read(&link).unwrap();
    save_snapshot(
        &edit("new"),
        &expected,
        SaveOptions::default(),
        &cancelled(),
        |_| {},
    )
    .unwrap();
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"new");
    let expected = DiskVersion::read(&link).unwrap();
    let error = save_snapshot(
        &edit("ours"),
        &expected,
        SaveOptions::default(),
        &cancelled(),
        |p| {
            if p.phase == SavePhase::Committing {
                fs::remove_file(&link).unwrap();
                symlink("other", &link).unwrap();
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, SaveFailureKind::Conflict);
    assert_eq!(fs::read(&target).unwrap(), b"new");
    assert_eq!(fs::read(&other).unwrap(), b"other");
}

#[cfg(unix)]
#[test]
fn permissions_and_extended_attributes_survive_both_save_modes() {
    use rustix::fs::{XattrFlags, fgetxattr, fsetxattr};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata");
    for atomic in [true, false] {
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        let file = File::open(&path).unwrap();
        fsetxattr(
            &file,
            c"user.plastext-test",
            b"metadata",
            XattrFlags::empty(),
        )
        .unwrap();
        drop(file);
        save_snapshot(
            &edit("new"),
            &DiskVersion::read(&path).unwrap(),
            SaveOptions { atomic },
            &cancelled(),
            |_| {},
        )
        .unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o750);
        let mut bytes = [0; 8];
        assert_eq!(
            fgetxattr(&file, c"user.plastext-test", &mut bytes[..]).unwrap(),
            8
        );
        assert_eq!(&bytes, b"metadata");
    }
}

#[test]
fn another_save_lock_prevents_writing_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, b"original").unwrap();
    let version = DiskVersion::read(&path).unwrap();
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    owner.lock().unwrap();
    let error = save_snapshot(
        &edit("new"),
        &version,
        SaveOptions::default(),
        &cancelled(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error.kind, SaveFailureKind::Io);
    assert!(!error.target_may_have_changed);
    drop(owner);
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert_eq!(entries(dir.path()), 1);
}

#[cfg(unix)]
#[test]
fn special_files_are_rejected_without_waiting_for_a_fifo_writer() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        DiskVersion::read(&fifo).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[cfg(target_os = "macos")]
#[test]
fn atomic_save_preserves_native_acl_entries() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("acl");
    fs::write(&path, b"original").unwrap();
    assert!(
        Command::new("chmod")
            .args(["+a", "everyone allow read"])
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let acl = |path: &Path| {
        let output = Command::new("ls").arg("-le").arg(path).output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let before = acl(&path);
    assert!(!before.is_empty());
    save_snapshot(
        &edit("new"),
        &DiskVersion::read(&path).unwrap(),
        SaveOptions::default(),
        &cancelled(),
        |_| {},
    )
    .unwrap();
    assert_eq!(acl(&path), before);
}

#[cfg(target_os = "macos")]
#[test]
fn oversized_resource_fork_fails_before_replacing_the_file() {
    use rustix::fs::{XattrFlags, fsetxattr};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource");
    fs::write(&path, b"original").unwrap();
    fsetxattr(
        File::open(&path).unwrap(),
        c"com.apple.ResourceFork",
        &vec![b'r'; 8 * 1024 * 1024 + 1],
        XattrFlags::empty(),
    )
    .unwrap();
    let error = save_snapshot(
        &edit("new"),
        &DiskVersion::read(&path).unwrap(),
        SaveOptions::default(),
        &cancelled(),
        |_| {},
    )
    .unwrap_err();
    assert!(!error.target_may_have_changed);
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert_eq!(entries(dir.path()), 1);
}

#[test]
fn encoded_saves_use_output_length_and_conversion_failure_never_reaches_commit() {
    use crate::{LineEnding, TextEncoding, TextFormat};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("converted.txt");
    for atomic in [true, false] {
        fs::write(&path, b"original").unwrap();
        let mut buffer = EditBuffer::from_bytes("a\nb\r\nc\r😀");
        buffer.set_format(
            TextFormat::new(TextEncoding::UTF16_LE, true)
                .unwrap()
                .with_line_ending(Some(LineEnding::CrLf)),
        );
        let receipt = save_snapshot(
            &buffer.snapshot(),
            &DiskVersion::read(&path).unwrap(),
            SaveOptions { atomic },
            &cancelled(),
            |_| {},
        )
        .unwrap();
        assert!(receipt.version.is_current().unwrap());
        let mut expected = vec![0xff, 0xfe];
        for word in "a\r\nb\r\nc\r\n😀".encode_utf16() {
            expected.extend(word.to_le_bytes());
        }
        assert_eq!(fs::read(&path).unwrap(), expected);
        buffer.set_format(
            TextFormat::new(TextEncoding::for_label("windows-1252").unwrap(), false).unwrap(),
        );
        let failure = save_snapshot(
            &buffer.snapshot(),
            &receipt.version,
            SaveOptions { atomic },
            &cancelled(),
            |p| assert_eq!(p.phase, SavePhase::Writing),
        )
        .unwrap_err();
        assert!(!failure.target_may_have_changed);
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(entries(dir.path()), 1);
        // A shorter encoded output must also truncate in-place correctly.
        buffer
            .replace_text(0..buffer.snapshot().len(), "x\n", UndoGroup::New)
            .unwrap();
        save_snapshot(
            &buffer.snapshot(),
            &receipt.version,
            SaveOptions { atomic },
            &cancelled(),
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"x\n");
    }
}

#[test]
fn dropping_a_save_lock_unlocks_even_when_another_descriptor_survives() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let locked = SaveLock::new(platform::open(file.path(), true).unwrap()).unwrap();
    let duplicate = locked.try_clone().unwrap();
    assert!(SaveLock::new(platform::open(file.path(), true).unwrap()).is_err());
    drop(locked);
    let next = SaveLock::new(platform::open(file.path(), true).unwrap()).unwrap();
    drop(next);
    drop(duplicate);
}
