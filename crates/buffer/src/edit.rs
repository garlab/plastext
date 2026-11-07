//! Headless editing model. Preparing a file blocks; edits, snapshots and reads
//! of cached ranges do not perform I/O. UI input and saving are separate layers.
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Write},
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::line_summary::LineSummary;
use bytes::Bytes;

use crate::{
    ByteSource, EncodingHint, FileSource, LineEnding, LineEndings, OpenOptions, ReadError,
    TextFormat,
    piece_tree::{self, Tree},
    unicode_summary::UnicodeSummary,
};

const HISTORY_BUDGET: u64 = 128 * 1024 * 1024;

/// A cheap immutable document version. Clones share the tree and byte storage.
/// Background consumers retain their version even while the buffer is edited.
#[derive(Clone)]
pub struct TextSnapshot {
    pub(super) root: Tree,
    owner: Arc<()>,
    revision: u64,
    format: TextFormat,
}

impl TextSnapshot {
    pub fn same_version(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner) && self.revision == other.revision
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn format(&self) -> TextFormat {
        self.format
    }
    pub fn line_endings(&self) -> LineEndings {
        self.root
            .as_ref()
            .map_or_else(LineEndings::default, |node| node.lines.counts)
    }
    pub fn preferred_line_ending(&self) -> LineEnding {
        self.format
            .line_ending()
            .unwrap_or_else(|| self.line_endings().preferred())
    }

    /// Editor lines are zero-based. LF, CRLF and CR each end one line.
    /// A trailing terminator starts an empty, editable line.
    pub fn line_count(&self) -> u64 {
        self.root
            .as_ref()
            .map_or(1, |node| node.lines.counts.count() + 1)
    }

    /// Cached-only line lookup. On Missing, load that snapshot-relative range
    /// on a background worker and retry. EOF is a valid caret position.
    pub fn line_at(&self, offset: u64) -> Result<u64, ReadError> {
        assert!(offset <= self.len(), "offset outside snapshot");
        piece_tree::line_at(&self.root, offset)
    }

    pub fn line_start(&self, line: u64) -> Result<Option<u64>, ReadError> {
        piece_tree::line_start(&self.root, line)
    }

    /// Load original-file pieces overlapping a snapshot-relative range. Blocks:
    /// call off the UI thread. The original source retains its bounded cache.
    pub fn load(&self, range: Range<u64>) -> io::Result<()> {
        self.check_range(&range);
        piece_tree::visit(&self.root, range, 0, &mut |piece, local, _| {
            piece.load(local)
        })
    }

    /// The first uncached piece in a range, without allocating copied bytes.
    pub fn missing(&self, range: Range<u64>) -> Option<Range<u64>> {
        self.check_range(&range);
        piece_tree::visit(&self.root, range, 0, &mut |piece, local, base| {
            if let Some(range) = piece.missing(local) {
                Err(base + range.start..base + range.end)
            } else {
                Ok(())
            }
        })
        .err()
    }

    /// Read working bytes on a worker without populating the viewport cache.
    pub fn read_uncached(&self, range: Range<u64>, output: &mut Vec<u8>) -> io::Result<()> {
        self.check_range(&range);
        piece_tree::visit(&self.root, range, 0, &mut |piece, local, _| {
            let start = output.len();
            output.resize(start + (local.end - local.start) as usize, 0);
            piece.read_uncached(local, &mut output[start..])
        })
    }

    /// Stream this version using its output encoding/newline settings, with
    /// bounded scratch space and no read-cache pollution. Progress counts UTF-8
    /// input bytes, not encoded output bytes. Blocks: run on a worker.
    pub fn write_to(
        &self,
        output: &mut impl Write,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> io::Result<()> {
        check_cancel(cancel)?;
        let mut output = crate::serialize::Serializer::new(output, self.format)?;
        let mut buffer = vec![0; piece_tree::LEAF_BYTES];
        let mut written = 0;
        progress(0, self.len());
        piece_tree::visit(&self.root, 0..self.len(), 0, &mut |piece, range, _| {
            check_cancel(cancel)?;
            let len = range.end - range.start;
            piece.serialize(range, &mut buffer, &mut output)?;
            written += len;
            progress(written, self.len());
            Ok::<_, io::Error>(())
        })?;
        check_cancel(cancel)?;
        output.finish()
    }

    fn check_range(&self, range: &Range<u64>) {
        assert!(
            range.start <= range.end && range.end <= self.len(),
            "range outside snapshot"
        );
    }
}

impl ByteSource for TextSnapshot {
    fn len(&self) -> u64 {
        piece_tree::len(&self.root)
    }

    fn bytes(&self, range: Range<u64>) -> Result<Bytes, ReadError> {
        self.check_range(&range);
        piece_tree::bytes(&self.root, range)
    }
}

/// Whether this change starts an undo transaction or extends a typing run.
/// The input layer decides boundaries (cursor moves, pauses, IME commits, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoGroup {
    New,
    Continue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidRange,
    LengthOverflow,
    Read(ReadError),
}

/// Versions removed from history, retained for destruction on a worker.
pub struct RetiredHistory {
    _changes: Vec<Change>,
}

struct Change {
    before: TextSnapshot,
    after: TextSnapshot,
    charge: u64,
    edits: Vec<EditDelta>,
}

#[derive(Clone, Debug)]
pub struct EditDelta {
    pub range: Range<u64>,
    pub inserted: u64,
}

impl EditDelta {
    pub fn inverse(&self) -> Self {
        Self {
            range: self.range.start..self.range.start + self.inserted,
            inserted: self.range.end - self.range.start,
        }
    }
}

/// Persistent piece-tree buffer with bounded undo/redo. It never writes to the
/// source path. A large file's original bytes live in a private disk snapshot;
/// editing cannot observe later external rewrites or truncation.
pub struct EditBuffer {
    current: TextSnapshot,
    // Retain the original independently of history. Its read cache has its own
    // budget, and deleting everything must not hide its cost in undo history.
    original: Arc<FileSource>,
    chunked: bool,
    next_revision: u64,
    saved_revision: u64,
    undo: VecDeque<Change>,
    redo: VecDeque<Change>,
    history_bytes: u64,
    history_budget: u64,
    group_open: bool,
    retired: Option<Vec<Change>>,
}

impl EditBuffer {
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        let source = Arc::new(FileSource::from_bytes(bytes));
        let leaves = summaries(source.in_memory().unwrap());
        Self::from_original(source, &leaves)
    }

    /// Prepare an immutable editable original off the UI thread. Large files
    /// are copied to an anonymous temporary file with bounded working memory.
    /// Progress is (copied bytes, total bytes). Cancellation, I/O errors and a
    /// detected concurrent rewrite discard the temporary file and return no buffer.
    pub fn open(
        path: &Path,
        options: OpenOptions,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> io::Result<Self> {
        check_cancel(cancel)?;
        let mut input = File::open(path)?;
        let before = crate::file_stamp::read(&input)?;
        let len = before.len;
        let mut disk = (len > options.large_file_threshold)
            .then(tempfile::tempfile)
            .transpose()?;
        let mut memory = Vec::new();
        let mut leaves = Vec::new();
        let mut chunk = vec![0; crate::BLOCK_SIZE as usize];
        let mut copied = 0;
        progress(0, len);
        while copied < len {
            check_cancel(cancel)?;
            let take = (len - copied).min(chunk.len() as u64) as usize;
            input.read_exact(&mut chunk[..take])?;
            if let Some(file) = &mut disk {
                file.write_all(&chunk[..take])?;
            } else {
                memory.extend_from_slice(&chunk[..take]);
            }
            leaves.extend(summaries(&chunk[..take]));
            copied += take as u64;
            progress(copied, len);
        }
        check_cancel(cancel)?;
        // Compare both the opened file and the current path: a rename can leave
        // the old descriptor unchanged while replacing the user's document.
        if crate::file_stamp::read(&input)? != before
            || crate::file_stamp::read_path(path)? != before
        {
            return Err(io::Error::other(
                "the file changed while preparing edit mode",
            ));
        }
        let source = match disk {
            Some(file) => FileSource::from_file(file, options)?,
            None => FileSource::from_bytes(memory),
        };
        Ok(Self::from_original(Arc::new(source), &leaves))
    }

    fn from_original(
        source: Arc<FileSource>,
        leaves: &[(u64, LineSummary, UnicodeSummary)],
    ) -> Self {
        Self {
            current: TextSnapshot {
                root: piece_tree::original(source.clone(), leaves, 0),
                owner: Arc::new(()),
                revision: 0,
                format: TextFormat::default(),
            },
            chunked: !source.is_in_memory(),
            original: source,
            next_revision: 1,
            saved_revision: 0,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            history_bytes: 0,
            history_budget: HISTORY_BUDGET,
            group_open: false,
            retired: None,
        }
    }

    /// Decode an immutable original into the editor's UTF-8 coordinate space.
    /// Blocks off the UI thread. Progress reserves half for copying and half
    /// for decoding (skipped for UTF-8), measured in original-source bytes.
    pub fn open_text(
        path: &Path,
        options: OpenOptions,
        hint: EncodingHint,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> io::Result<Self> {
        let mut size = 0;
        let buffer = Self::open(path, options, cancel, |done, total| {
            size = total;
            progress(done, total.saturating_mul(2));
        })?;
        let total = size.saturating_mul(2);
        let buffer = buffer.decode(options, hint, cancel, &mut |done, _| {
            progress(size.saturating_add(done), total);
        })?;
        check_cancel(cancel)?;
        progress(total, total);
        Ok(buffer)
    }

    /// Blocking decoding for supplied bytes; run off the UI thread.
    pub fn from_encoded_bytes(bytes: impl Into<Bytes>, hint: EncodingHint) -> io::Result<Self> {
        Self::from_bytes(bytes).decode(
            OpenOptions::default(),
            hint,
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
    }

    fn decode(
        mut self,
        options: OpenOptions,
        hint: EncodingHint,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> io::Result<Self> {
        check_cancel(cancel)?;
        let (format, skip) = crate::encoding::detect(&self.original, hint)?;
        if format.encoding() == crate::TextEncoding::UTF8 {
            if skip != 0 {
                self.original.load(0..skip)?;
                self.current.root = piece_tree::split(self.current.root, skip)
                    .map_err(|_| io::Error::other("cannot read byte-order mark"))?
                    .1;
            }
        } else {
            let (source, leaves) = crate::encoding::DecodedSource::prepare(
                self.original.clone(),
                format.encoding(),
                skip,
                options,
                cancel,
                progress,
            )?;
            self.chunked |= !source.text.is_in_memory();
            self.current.root = piece_tree::decoded(source, &leaves);
        }
        self.current.format = format;
        Ok(self)
    }

    /// An undoable output-setting transaction. Does no I/O or text rewriting.
    pub fn set_format(&mut self, format: TextFormat) -> bool {
        if self.current.format == format {
            return false;
        }
        let before = self.current.clone();
        let mut after = before.clone();
        after.format = format;
        after.revision = self.next_revision;
        self.record(
            before,
            after,
            size_of::<Change>() as u64,
            None,
            UndoGroup::New,
        );
        self.break_undo_group();
        true
    }

    pub fn snapshot(&self) -> TextSnapshot {
        self.current.clone()
    }
    pub fn is_dirty(&self) -> bool {
        self.current.revision != self.saved_revision
    }
    pub fn is_chunked(&self) -> bool {
        self.chunked
    }
    /// Retain evicted history for worker-thread destruction. Once enabled,
    /// callers must drain `take_retired_history` after mutations and budget changes.
    pub fn defer_history_drop(&mut self) {
        self.retired.get_or_insert_with(Vec::new);
    }
    pub fn take_retired_history(&mut self) -> Option<RetiredHistory> {
        self.retired
            .as_mut()
            .filter(|retired| !retired.is_empty())
            .map(|retired| RetiredHistory {
                _changes: std::mem::take(retired),
            })
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Conservative history charge for added/removed pieces and copied tree
    /// paths. Shared allocations can be counted more than once. Original bytes,
    /// the current document, allocator overhead and caller-held snapshots have
    /// separate lifetimes and are not governed by this history budget.
    pub fn history_bytes(&self) -> u64 {
        self.history_bytes
    }

    pub fn set_history_budget(&mut self, bytes: u64) {
        self.history_budget = bytes;
        self.trim_history();
    }

    pub fn break_undo_group(&mut self) {
        self.group_open = false;
    }

    /// Record a successfully saved version, even if typing continued while the
    /// save ran. Does not clear undo history or mark later edits clean. Rejects
    /// snapshots from another buffer. This performs no disk I/O.
    pub fn mark_saved(&mut self, saved: &TextSnapshot) -> bool {
        if !Arc::ptr_eq(&saved.owner, &self.current.owner) {
            return false;
        }
        self.saved_revision = saved.revision;
        self.break_undo_group();
        true
    }

    /// Replace raw bytes atomically. The caller's text layer must supply UTF-8
    /// or grapheme boundaries when editing characters. Invalid original bytes
    /// are preserved exactly. Failure leaves content, history and dirty state
    /// unchanged; Missing ranges use the current snapshot's coordinates.
    pub fn replace(
        &mut self,
        range: Range<u64>,
        bytes: &[u8],
        group: UndoGroup,
    ) -> Result<(), EditError> {
        if range.start > range.end || range.end > self.current.len() {
            return Err(EditError::InvalidRange);
        }
        self.current
            .len()
            .checked_sub(range.end - range.start)
            .and_then(|len| len.checked_add(bytes.len() as u64))
            .ok_or(EditError::LengthOverflow)?;
        if range.is_empty() && bytes.is_empty() {
            return Ok(());
        }
        let before = self.current.clone();
        // Split the end first, keeping both error ranges in document coordinates.
        let (prefix, suffix) =
            piece_tree::split(before.root.clone(), range.end).map_err(EditError::Read)?;
        let (prefix, removed) = piece_tree::split(prefix, range.start).map_err(EditError::Read)?;
        let added = piece_tree::added(bytes);
        let payload = piece_tree::cost(&removed).saturating_add(piece_tree::cost(&added));
        let root = piece_tree::join(piece_tree::join(prefix, added), suffix);
        // A split/join copies paths and can rotate on each level. Charge both
        // directions, including Arc bookkeeping, rather than counting only text.
        let heights = u64::from(before.root.as_ref().map_or(0, |n| n.height))
            + u64::from(root.as_ref().map_or(0, |n| n.height))
            + 1;
        let charge = payload
            .saturating_add(heights * 8 * (size_of::<piece_tree::Node>() as u64 + 32))
            .saturating_add(2 * size_of::<EditDelta>() as u64);
        let edit = EditDelta {
            range,
            inserted: bytes.len() as u64,
        };
        let after = TextSnapshot {
            root,
            owner: before.owner.clone(),
            revision: self.next_revision,
            format: before.format,
        };
        self.record(before, after, charge, Some(edit), group);
        Ok(())
    }

    fn record(
        &mut self,
        before: TextSnapshot,
        after: TextSnapshot,
        charge: u64,
        edit: Option<EditDelta>,
        group: UndoGroup,
    ) {
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .expect("document revision exhausted");
        let coalesce = group == UndoGroup::Continue && self.group_open && self.redo.is_empty();
        for change in self.redo.drain(..) {
            self.history_bytes -= change.charge;
            if let Some(retired) = &mut self.retired {
                retired.push(change);
            }
        }
        self.history_bytes = self.history_bytes.saturating_add(charge);
        if let Some(previous) = self.undo.back_mut().filter(|_| coalesce) {
            previous.after = after.clone();
            previous.charge = previous.charge.saturating_add(charge);
            if let Some(edit) = edit {
                previous.edits.push(edit);
            }
        } else {
            self.undo.push_back(Change {
                before,
                after: after.clone(),
                charge,
                edits: edit.into_iter().collect(),
            });
        }
        self.current = after;
        self.group_open = true;
        self.trim_history();
    }

    pub fn undo(&mut self) -> bool {
        self.undo_with(|_| {})
    }

    pub(super) fn undo_with(&mut self, changed: impl FnOnce(&[EditDelta])) -> bool {
        self.break_undo_group();
        let Some(change) = self.undo.pop_back() else {
            return false;
        };
        self.current = change.before.clone();
        changed(&change.edits);
        self.redo.push_back(change);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.redo_with(|_| {})
    }

    pub(super) fn redo_with(&mut self, changed: impl FnOnce(&[EditDelta])) -> bool {
        self.break_undo_group();
        let Some(change) = self.redo.pop_back() else {
            return false;
        };
        self.current = change.after.clone();
        changed(&change.edits);
        self.undo.push_back(change);
        true
    }

    fn trim_history(&mut self) {
        while self.history_bytes > self.history_budget {
            let Some(change) = self.undo.pop_front().or_else(|| self.redo.pop_front()) else {
                break;
            };
            self.history_bytes -= change.charge;
            if let Some(retired) = &mut self.retired {
                retired.push(change);
            }
        }
    }
}

fn summaries(bytes: &[u8]) -> Vec<(u64, LineSummary, UnicodeSummary)> {
    bytes
        .chunks(piece_tree::LEAF_BYTES)
        .map(|chunk| {
            (
                chunk.len() as u64,
                LineSummary::new(chunk),
                UnicodeSummary::new(chunk),
            )
        })
        .collect()
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
