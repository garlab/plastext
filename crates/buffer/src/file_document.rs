//! Worker jobs and UI-owned completion checks for an editable file. Callers
//! debounce watcher events and schedule jobs; this module starts no threads.
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Weak, atomic::AtomicBool},
};

use crate::{
    DiskVersion, DocumentSession, DocumentSnapshot, EditBuffer, EncodingHint, OpenOptions,
    SaveFailure, SaveFailureKind, SaveOptions, SaveProgress, SaveReceipt, save_snapshot,
};

pub struct FileDocument {
    session: DocumentSession,
    disk: Option<DiskVersion>,
    epoch: u64,
    saving: Weak<()>,
    conflict: bool,
    encoding_hint: EncodingHint,
}

impl FileDocument {
    /// An empty editable document with no filesystem backing or side effects.
    pub fn untitled() -> Self {
        Self {
            session: DocumentSession::new(EditBuffer::from_bytes(Vec::new())),
            disk: None,
            epoch: 0,
            saving: Weak::new(),
            conflict: false,
            encoding_hint: EncodingHint::Auto,
        }
    }
    /// Blocking preparation, including consistency checks; run on a worker.
    pub fn open(
        path: &Path,
        options: OpenOptions,
        cancel: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> io::Result<Self> {
        Self::open_with_encoding(path, options, EncodingHint::Auto, cancel, progress)
    }

    pub fn open_with_encoding(
        path: &Path,
        options: OpenOptions,
        hint: EncodingHint,
        cancel: &AtomicBool,
        progress: impl FnMut(u64, u64),
    ) -> io::Result<Self> {
        let disk = DiskVersion::read(path)?;
        let buffer = open_version(&disk, options, hint, cancel, progress)?;
        Ok(Self {
            session: DocumentSession::new(buffer),
            disk: Some(disk),
            epoch: 0,
            saving: Weak::new(),
            conflict: false,
            encoding_hint: hint,
        })
    }

    pub fn session(&self) -> &DocumentSession {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut DocumentSession {
        &mut self.session
    }
    pub fn disk_version(&self) -> Option<&DiskVersion> {
        self.disk.as_ref()
    }
    pub fn has_conflict(&self) -> bool {
        self.conflict
    }
    /// A watcher-paired rename, verified off-thread by identity. Retain the old
    /// content stamp so edits made during the move still cause a conflict.
    pub fn follow_rename(&mut self, expected: &DiskVersion, current: &DiskVersion) -> bool {
        if self.is_saving() || self.disk.as_ref() != Some(expected) || !expected.same_file(current)
        {
            return false;
        }
        self.disk = Some(expected.relocated(current));
        self.advance();
        true
    }
    pub fn is_saving(&self) -> bool {
        self.saving.strong_count() != 0
    }

    /// None means the document is untitled or a save job/completion is still
    /// held. Dropping an unstarted job releases the slot. Editing remains
    /// available during saves.
    pub fn begin_save(&mut self, options: SaveOptions) -> Option<SaveJob> {
        let expected = self.disk.clone()?;
        self.save_against(expected.path().to_owned(), Some(expected), options)
    }

    /// First save of an untitled document. An existing destination must be a
    /// version reviewed by the caller; a missing destination is never clobbered.
    pub fn begin_save_as(
        &mut self,
        path: PathBuf,
        expected: Option<DiskVersion>,
        options: SaveOptions,
    ) -> Result<Option<SaveJob>, SaveFailure> {
        if self.disk.is_some()
            || expected
                .as_ref()
                .is_some_and(|version| version.path() != path)
        {
            return Err(stale_save());
        }
        Ok(self.save_against(path, expected, options))
    }

    /// Explicit conflict resolution: the caller must obtain the user's choice
    /// to replace this reviewed disk version. A later disk change still aborts.
    pub fn begin_overwrite(
        &mut self,
        reviewed: DiskVersion,
        options: SaveOptions,
    ) -> Result<Option<SaveJob>, SaveFailure> {
        if self
            .disk
            .as_ref()
            .is_none_or(|disk| reviewed.path() != disk.path())
        {
            return Err(stale_save());
        }
        Ok(self.save_against(reviewed.path().to_owned(), Some(reviewed), options))
    }

    fn save_against(
        &mut self,
        path: PathBuf,
        expected: Option<DiskVersion>,
        options: SaveOptions,
    ) -> Option<SaveJob> {
        if self.is_saving() {
            return None;
        }
        self.advance();
        let lease = Arc::new(());
        self.saving = Arc::downgrade(&lease);
        Some(SaveJob {
            lease,
            snapshot: self.session.snapshot(),
            expected,
            path,
            options,
        })
    }

    /// No I/O. Only this saved snapshot becomes the checkpoint; newer typing
    /// remains dirty. A failed/uncertain save never changes the checkpoint.
    /// The caller retains the completion for off-thread snapshot retirement.
    pub fn finish_save(&mut self, completion: &SaveCompletion) -> Result<SaveReceipt, SaveFailure> {
        if !self
            .saving
            .upgrade()
            .is_some_and(|lease| Arc::ptr_eq(&lease, &completion.lease))
        {
            return Err(stale_save());
        }
        self.saving = Weak::new();
        self.advance();
        match &completion.result {
            Ok(receipt) => {
                if !self.session.mark_saved(&completion.snapshot) {
                    return Err(stale_save());
                }
                self.disk = Some(receipt.version.clone());
                self.encoding_hint =
                    EncodingHint::Explicit(completion.snapshot.text().format().encoding());
                self.conflict = false;
                Ok(receipt.clone())
            }
            Err(failure) => {
                if failure.kind == SaveFailureKind::Conflict || failure.target_may_have_changed {
                    self.conflict = true;
                }
                Err(failure.clone())
            }
        }
    }

    /// Schedule after coalescing watcher events, or for periodic polling.
    /// Untitled documents have no disk version to check and return None.
    pub fn external_check(&mut self, options: OpenOptions) -> Option<ExternalCheck> {
        let expected = self.disk.clone()?;
        self.advance();
        Some(ExternalCheck {
            expected,
            base: self.session.snapshot(),
            epoch: self.epoch,
            clean: !self.session.is_dirty(),
            discard_edits: false,
            force_reload: false,
            encoding_hint: self.encoding_hint,
            options,
            positions: self.session.reload_positions(),
        })
    }

    /// Explicit discard/reload, for a user-selected conflict resolution. Edits
    /// made after this request still invalidate its result.
    pub fn reload_discarding_edits(&mut self, options: OpenOptions) -> Option<ExternalCheck> {
        Some(ExternalCheck {
            clean: true,
            discard_edits: true,
            ..self.external_check(options)?
        })
    }

    /// Reinterpret a clean document on a worker. Never implicitly discard dirty
    /// edits; edits or newer requests made while decoding invalidate this job.
    pub fn reopen_with_encoding(
        &mut self,
        hint: EncodingHint,
        options: OpenOptions,
    ) -> Option<ExternalCheck> {
        if self.session.is_dirty() || self.is_saving() {
            return None;
        }
        Some(ExternalCheck {
            force_reload: true,
            encoding_hint: hint,
            ..self.external_check(options)?
        })
    }

    /// No I/O. Return unused/replaced buffers so callers can retire potentially
    /// large histories on a worker rather than dropping them on the UI thread.
    /// The caller also retires the observation and its snapshot on that worker.
    pub fn finish_external(&mut self, observation: &mut ExternalObservation) -> ExternalResult {
        let buffer = observation.buffer.take();
        let ExternalObservation {
            expected,
            base,
            epoch,
            observed,
            discard_edits,
            ..
        } = &*observation;
        let state = if self.is_saving() {
            ExternalState::Deferred
        } else if *epoch != self.epoch || Some(expected) != self.disk.as_ref() {
            ExternalState::Stale
        } else {
            match observed {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.conflict = true;
                    ExternalState::Missing
                }
                Err(error) => ExternalState::Failed(error.to_string()),
                Ok(current)
                    if Some(current) == self.disk.as_ref()
                        && !discard_edits
                        && !observation.force_reload =>
                {
                    self.conflict = false;
                    ExternalState::Unchanged
                }
                Ok(current) => {
                    if self.session.is_dirty() && (!discard_edits || !self.session.is_current(base))
                    {
                        self.conflict = true;
                        ExternalState::Conflict
                    } else if let Some(buffer) = buffer {
                        return match self.session.reload(
                            base,
                            buffer,
                            *discard_edits,
                            observation.mapping.as_ref().unwrap(),
                        ) {
                            Ok(retired) => {
                                self.disk = Some(current.clone());
                                self.encoding_hint = observation.encoding_hint;
                                self.conflict = false;
                                self.advance();
                                ExternalResult {
                                    state: ExternalState::Reloaded,
                                    retired: Some(retired),
                                }
                            }
                            Err(retired) => ExternalResult {
                                state: ExternalState::Stale,
                                retired: Some(*retired),
                            },
                        };
                    } else {
                        ExternalState::Deferred
                    }
                }
            }
        };
        ExternalResult {
            state,
            retired: buffer,
        }
    }

    fn advance(&mut self) {
        self.epoch = self
            .epoch
            .checked_add(1)
            .expect("file document generation exhausted");
    }
}

fn stale_save() -> SaveFailure {
    SaveFailure {
        kind: SaveFailureKind::Stale,
        message: "save completion belongs to another operation".into(),
        recovery_path: None,
        target_may_have_changed: true,
    }
}

pub struct SaveJob {
    lease: Arc<()>,
    snapshot: DocumentSnapshot,
    expected: Option<DiskVersion>,
    path: PathBuf,
    options: SaveOptions,
}
impl SaveJob {
    pub fn run(self, cancel: &AtomicBool, progress: impl FnMut(SaveProgress)) -> SaveCompletion {
        let result = if let Some(expected) = &self.expected {
            save_snapshot(
                self.snapshot.text(),
                expected,
                self.options,
                cancel,
                progress,
            )
        } else {
            crate::save::create_snapshot(self.snapshot.text(), &self.path, cancel, progress)
        };
        SaveCompletion {
            lease: self.lease,
            snapshot: self.snapshot,
            result,
        }
    }
}

#[must_use = "apply the save result before retiring its snapshot on a worker"]
pub struct SaveCompletion {
    lease: Arc<()>,
    snapshot: DocumentSnapshot,
    result: Result<SaveReceipt, SaveFailure>,
}
impl SaveCompletion {
    /// Inspect failures (including retained recovery paths) before application.
    pub fn result(&self) -> Result<&SaveReceipt, &SaveFailure> {
        self.result.as_ref()
    }
}

pub struct ExternalCheck {
    positions: Vec<u64>,
    expected: DiskVersion,
    base: DocumentSnapshot,
    epoch: u64,
    clean: bool,
    discard_edits: bool,
    force_reload: bool,
    encoding_hint: EncodingHint,
    options: OpenOptions,
}
impl ExternalCheck {
    /// A clean document prepares a replacement on the worker. A dirty document
    /// only probes the disk version, avoiding unnecessary full-file copies.
    pub fn run(self, cancel: &AtomicBool, progress: impl FnMut(u64, u64)) -> ExternalObservation {
        let mut buffer = None;
        let mut mapping = None;
        let observed = (|| {
            crate::edit::check_cancel(cancel)?;
            let current = DiskVersion::read(self.expected.path())?;
            if (current != self.expected || self.discard_edits || self.force_reload) && self.clean {
                buffer = Some(open_version(
                    &current,
                    self.options,
                    self.encoding_hint,
                    cancel,
                    progress,
                )?);
                mapping = Some(crate::reload::Mapping::prepare(
                    self.base.text(),
                    &buffer.as_ref().unwrap().snapshot(),
                    &self.positions,
                    cancel,
                )?);
            }
            Ok(current)
        })();
        ExternalObservation {
            expected: self.expected,
            base: self.base,
            epoch: self.epoch,
            observed,
            buffer,
            discard_edits: self.discard_edits,
            force_reload: self.force_reload,
            encoding_hint: self.encoding_hint,
            mapping,
        }
    }
}

pub struct ExternalObservation {
    mapping: Option<crate::reload::Mapping>,
    expected: DiskVersion,
    base: DocumentSnapshot,
    epoch: u64,
    observed: io::Result<DiskVersion>,
    buffer: Option<EditBuffer>,
    discard_edits: bool,
    force_reload: bool,
    encoding_hint: EncodingHint,
}

impl ExternalObservation {
    pub fn disk_version(&self) -> Option<&DiskVersion> {
        self.observed.as_ref().ok()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExternalState {
    Unchanged,
    Reloaded,
    Conflict,
    Missing,
    Deferred,
    Stale,
    Failed(String),
}

pub struct ExternalResult {
    pub state: ExternalState,
    pub retired: Option<EditBuffer>,
}

fn open_version(
    disk: &DiskVersion,
    options: OpenOptions,
    hint: EncodingHint,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> io::Result<EditBuffer> {
    let buffer = EditBuffer::open_text(disk.target(), options, hint, cancel, progress)?;
    if !disk.is_current()? {
        return Err(io::Error::other("file changed while preparing reload"));
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests;
