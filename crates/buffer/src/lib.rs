//! Piece-tree text buffer, chunked file source, line index, undo, encoding and saving (spec §5.2).
//!
//! So far: [`FileSource`] reads files of any size without loading them whole,
//! [`LineIndex`] maps lines to offsets as it scans in the background, and
//! [`rows`] splits text into display rows of bounded length.

mod line_index;
pub mod rows;
mod source;

pub use line_index::{LineIndex, Progress};
pub use source::{BLOCK_SIZE, ByteSource, FileSource, OpenOptions, ReadError};
