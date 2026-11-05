//! Worker-side reads shared by disk previews and immutable editing versions.
use plastext_buffer::{ByteSource, FileSource, TextSnapshot};
use std::{
    io,
    ops::{ControlFlow, Range},
    sync::Arc,
};

/// Append a bounded source range without filling an editor's viewport cache.
/// Reads may block and must run on a worker. Callers choose the range size.
pub trait DocumentSource: Send + Sync {
    fn byte_len(&self) -> u64;
    fn read_range(&self, range: Range<u64>, output: &mut Vec<u8>) -> io::Result<()>;
}
impl DocumentSource for FileSource {
    fn byte_len(&self) -> u64 {
        self.len()
    }
    fn read_range(&self, range: Range<u64>, output: &mut Vec<u8>) -> io::Result<()> {
        self.scan(range, |chunk| {
            output.extend_from_slice(chunk);
            ControlFlow::Continue(())
        })
    }
}
impl DocumentSource for TextSnapshot {
    fn byte_len(&self) -> u64 {
        self.len()
    }
    fn read_range(&self, range: Range<u64>, output: &mut Vec<u8>) -> io::Result<()> {
        self.read_uncached(
            range.start.min(self.len())..range.end.min(self.len()),
            output,
        )
    }
}
impl<T: DocumentSource + ?Sized> DocumentSource for Arc<T> {
    fn byte_len(&self) -> u64 {
        (**self).byte_len()
    }
    fn read_range(&self, range: Range<u64>, output: &mut Vec<u8>) -> io::Result<()> {
        (**self).read_range(range, output)
    }
}
