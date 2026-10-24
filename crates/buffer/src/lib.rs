//! Piece-tree text buffer, chunked file source, line index, undo, encoding and saving (spec §5.2).
//!
//! So far: [`FileSource`] reads files of any size without loading them whole,
//! [`LineIndex`] maps lines to offsets as it scans in the background, and
//! [`rows`] splits text into display rows of bounded length.

mod line_index;
pub mod rows;
mod source;

use std::{io, path::Path, sync::Arc};

pub use line_index::{LineIndex, Progress};
pub use source::{BLOCK_SIZE, ByteSource, FileSource, OpenOptions, ReadError};

/// Opens `path` for viewing. Small files are read and indexed right away;
/// large ones are indexed on a background thread. Blocks: call it off the main thread.
pub fn open(path: &Path, options: OpenOptions) -> io::Result<(Arc<FileSource>, Arc<LineIndex>)> {
    let source = Arc::new(FileSource::open(path, options)?);
    let index = if source.is_in_memory() {
        Arc::new(LineIndex::build(&source)?)
    } else {
        LineIndex::spawn(source.clone())
    };
    Ok((source, index))
}
