//! Persistent text buffers, bounded file reads and Unicode navigation (spec §5.2).
//!
//! [`FileSource`] provides bounded reads, [`LineIndex`] builds a sparse line map,
//! and [`rows`] splits text into bounded display rows. [`EditBuffer`] supplies
//! persistent byte edits, immutable snapshots and bounded undo/redo for M3.
//! [`DocumentSession`] shares that history across pane-local selections, with
//! indexed Unicode coordinates and resumable [`GraphemeSearch`] navigation.

mod edit;
mod file_document;
mod grapheme;
mod line_index;
mod piece_tree;
pub mod rows;
mod save;
mod session;
mod source;
mod text;
mod unicode_summary;

use std::{io, path::Path, sync::Arc};

pub use edit::{EditBuffer, EditError, TextSnapshot, UndoGroup};
pub use file_document::{
    ExternalCheck, ExternalObservation, ExternalResult, ExternalState, FileDocument,
    SaveCompletion, SaveJob,
};
pub use grapheme::{Direction, GraphemeSearch};
pub use line_index::{LineIndex, Progress};
pub use save::{
    DiskVersion, SaveFailure, SaveFailureKind, SaveOptions, SavePhase, SaveProgress, SaveReceipt,
    save_snapshot,
};
pub use session::{
    Anchor, CursorCommand, DocumentSession, DocumentSnapshot, PaneId, PaneSnapshot, PendingCommand,
    Selection, SessionError,
};
pub use source::{BLOCK_SIZE, ByteSource, FileSource, OpenOptions, ReadError};
pub use text::{Bias, TextError};

/// How much of a file [`looks_binary`] looks at.
pub const SNIFF_LEN: u64 = 8192;

/// Opens `path` for viewing. Small files are read and indexed right away;
/// large ones are indexed on a background thread. Blocks: call it off the main thread.
pub fn open(path: &Path, options: OpenOptions) -> io::Result<(Arc<FileSource>, Arc<LineIndex>)> {
    let source = Arc::new(FileSource::open(path, options)?);
    let index = LineIndex::for_source(source.clone())?;
    Ok((source, index))
}

/// Whether a file starting with `head` is binary rather than text (spec
/// TXT-9): it has a NUL byte, or over a tenth of it isn't valid UTF-8.
pub fn looks_binary(head: &[u8]) -> bool {
    if head.contains(&0) {
        return true;
    }
    let invalid: usize = head.utf8_chunks().map(|chunk| chunk.invalid().len()).sum();
    // A multibyte character cut off at the end of `head` isn't evidence.
    let invalid = invalid.saturating_sub(3);
    invalid * 10 > head.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_binary_from_text() {
        assert!(!looks_binary(b""));
        assert!(!looks_binary("plain text, é, 😀\n".as_bytes()));
        assert!(!looks_binary(&"é".as_bytes()[..1]), "a truncated character");
        assert!(!looks_binary(b"caf\xe9 cr\xe8me, mostly ASCII in Latin-1"));
        assert!(looks_binary(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"));
        assert!(looks_binary(&[
            0xff, 0xfe, 0xfd, 0xfc, 0x80, 0x81, 0x82, 0x90, 0x91, 0x92, 0x93, 0x94
        ]));
    }
}
