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
