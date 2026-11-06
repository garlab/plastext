//! Blocking save transactions for immutable snapshots. The document owner
//! schedules these jobs; no disk access belongs in its input/render callbacks.
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use crate::{TextSnapshot, edit::check_cancel};

mod platform;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskVersion {
    path: PathBuf,
    target: PathBuf,
    stamp: platform::Stamp,
}

impl DiskVersion {
    /// Resolve symlinks and capture identity, length, permissions and change
    /// timestamps. Blocks; use off the UI thread, including for watcher checks.
    pub fn read(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let target = fs::canonicalize(&path)?;
        let file = platform::open(&target, false)?;
        let stamp = platform::stamp(&file)?;
        if fs::canonicalize(&path)? != target
            || platform::stamp(&platform::open(&target, false)?)? != stamp
        {
            return Err(io::Error::other(
                "file changed while reading its disk version",
            ));
        }
        Ok(Self {
            path,
            target,
            stamp,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Check the current path's write access without changing the file.
    /// Blocks; call on a worker. This does not validate the captured version:
    /// snapshot preparation and saving perform their own consistency checks.
    pub fn is_writable(&self) -> io::Result<bool> {
        // An unrelated metadata update must not turn a permission probe into
        // a save conflict. Resolve again in case a symlink now points elsewhere.
        let target = fs::canonicalize(&self.path)?;
        match platform::open(&target, true) {
            Ok(_) => Ok(true),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    pub fn is_current(&self) -> io::Result<bool> {
        match Self::read(&self.path) {
            Ok(current) => Ok(current == *self),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn verify(&self, file: &File) -> Result<(), SaveFailure> {
        if platform::stamp(file)? != self.stamp || !self.is_current()? {
            return Err(SaveFailure::conflict());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SaveOptions {
    /// Hard-linked files always use in-place writes, regardless of this flag.
    pub atomic: bool,
}
impl Default for SaveOptions {
    fn default() -> Self {
        Self { atomic: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePhase {
    Writing,
    BackingUp,
    Committing,
}

#[derive(Clone, Copy, Debug)]
pub struct SaveProgress {
    pub phase: SavePhase,
    pub completed: u64,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFailureKind {
    Cancelled,
    Conflict,
    Io,
    Stale,
}

#[derive(Clone, Debug)]
pub struct SaveFailure {
    pub kind: SaveFailureKind,
    pub message: String,
    /// In-place failures retain the pre-save bytes for explicit recovery.
    pub recovery_path: Option<PathBuf>,
    /// A commit or durability check failed after writing could have begun.
    pub target_may_have_changed: bool,
}

impl SaveFailure {
    fn conflict() -> Self {
        Self {
            kind: SaveFailureKind::Conflict,
            message: "the file changed on disk".into(),
            recovery_path: None,
            target_may_have_changed: false,
        }
    }
}
impl From<io::Error> for SaveFailure {
    fn from(error: io::Error) -> Self {
        Self {
            kind: if error.kind() == io::ErrorKind::Interrupted {
                SaveFailureKind::Cancelled
            } else {
                SaveFailureKind::Io
            },
            message: error.to_string(),
            recovery_path: None,
            target_may_have_changed: false,
        }
    }
}
impl std::fmt::Display for SaveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for SaveFailure {}

#[derive(Clone, Debug)]
pub struct SaveReceipt {
    pub version: DiskVersion,
    pub in_place: bool,
}

/// Save an immutable version on a worker. Checks for detectable outside changes
/// before and after preparation; locks exclude cooperating writers only. No
/// portable filesystem API makes compare-version-and-replace indivisible.
/// Cancellation is honored until commit, then the write must finish. On an
/// uncertain outcome callers must retain dirty state and recheck the disk.
pub fn save_snapshot(
    snapshot: &TextSnapshot,
    expected: &DiskVersion,
    options: SaveOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(SaveProgress),
) -> Result<SaveReceipt, SaveFailure> {
    check_cancel(cancel)?;
    if !expected.is_current()? {
        return Err(SaveFailure::conflict());
    }
    // Request write permission without truncation, even for atomic replacement.
    let mut original = SaveLock::new(platform::open(&expected.target, true)?)?;
    expected.verify(&original)?;
    let parent = expected.target.parent().unwrap();
    let mut staged = tempfile::Builder::new()
        .prefix(".plastext-save-")
        .tempfile_in(parent)?;
    snapshot.write_to(staged.as_file_mut(), cancel, |completed, total| {
        progress(SaveProgress {
            phase: SavePhase::Writing,
            completed,
            total,
        });
    })?;
    let encoded_len = staged.as_file().metadata()?.len();
    let in_place = !options.atomic || expected.stamp.links > 1;
    let mut backup = if in_place {
        let mut backup = tempfile::Builder::new()
            .prefix(".plastext-recovery-")
            .tempfile_in(parent)?;
        copy_exact(
            &mut original,
            backup.as_file_mut(),
            expected.stamp.len,
            cancel,
            |completed, total| {
                progress(SaveProgress {
                    phase: SavePhase::BackingUp,
                    completed,
                    total,
                })
            },
        )?;
        backup.as_file().sync_all()?;
        Some(backup)
    } else {
        None
    };
    platform::copy_metadata(&original, staged.as_file(), cancel)?;
    staged.as_file().sync_all()?;
    if backup.is_some() {
        platform::sync_parent(&expected.target)?;
    }
    progress(SaveProgress {
        phase: SavePhase::Committing,
        completed: 0,
        total: encoded_len,
    });
    check_cancel(cancel)?;
    expected.verify(&original)?;

    // Preserve the backup's name before touching a hard-linked destination.
    // It survives process termination once an in-place commit has started.
    let recovery = backup
        .take()
        .map(|file| file.keep().map_err(|error| error.error))
        .transpose()?;
    let mut commit_started = false;
    let result = (|| -> Result<DiskVersion, SaveFailure> {
        let written = if in_place {
            staged.as_file_mut().rewind()?;
            original.rewind()?;
            commit_started = true;
            copy_exact(
                staged.as_file_mut(),
                &mut original,
                encoded_len,
                &AtomicBool::new(false),
                |completed, total| {
                    if completed != 0 {
                        progress(SaveProgress {
                            phase: SavePhase::Committing,
                            completed,
                            total,
                        });
                    }
                },
            )?;
            original.set_len(encoded_len)?;
            // Restore metadata that writing can clear (e.g. Unix mode bits).
            platform::copy_metadata(staged.as_file(), &original, &AtomicBool::new(false))?;
            original.sync_all()?;
            platform::stamp(&original)?
        } else {
            commit_started = true;
            let file = platform::replace(staged, &expected.target)?;
            file.sync_all()?;
            platform::stamp(&file)?
        };
        platform::sync_parent(&expected.target)?;
        let version = DiskVersion::read(&expected.path)?;
        if version.target != expected.target || version.stamp != written {
            return Err(SaveFailure::conflict());
        }
        Ok(version)
    })();
    match result {
        Ok(version) => {
            if let Some((file, path)) = recovery {
                drop(file);
                let _ = fs::remove_file(path);
            }
            Ok(SaveReceipt { version, in_place })
        }
        Err(mut failure) => {
            failure.target_may_have_changed = commit_started;
            if let Some((_, path)) = recovery {
                failure.recovery_path = Some(path);
            }
            Err(failure)
        }
    }
}

// Explicitly unlock on every exit. Closing our descriptor alone can leave a
// flock alive briefly when another thread forks a child during the save.
struct SaveLock(File);
impl SaveLock {
    fn new(file: File) -> io::Result<Self> {
        file.try_lock().map_err(io::Error::other)?;
        Ok(Self(file))
    }
}
impl std::ops::Deref for SaveLock {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl std::ops::DerefMut for SaveLock {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.0
    }
}
impl Drop for SaveLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn copy_exact(
    from: &mut File,
    to: &mut File,
    len: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<()> {
    let mut bytes = vec![0; 64 * 1024];
    let mut copied = 0;
    progress(0, len);
    while copied < len {
        check_cancel(cancel)?;
        let bytes = &mut bytes[..(len - copied).min(64 * 1024) as usize];
        from.read_exact(bytes)?;
        to.write_all(bytes)?;
        copied += bytes.len() as u64;
        progress(copied, len);
    }
    check_cancel(cancel)
}

#[cfg(test)]
mod tests;
