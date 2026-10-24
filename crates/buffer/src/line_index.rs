use std::{
    io,
    ops::ControlFlow,
    sync::{Arc, RwLock, Weak},
    thread,
};

use memchr::memchr_iter;

use crate::{ByteSource, FileSource, ReadError};

/// The index records one newline count per block of this many bytes.
const INDEX_BLOCK: u64 = 64 * 1024;

/// A sparse map between line numbers and byte offsets (spec TXT-5).
///
/// It stores the number of newlines before each [`INDEX_BLOCK`] boundary, so
/// it takes O(file size / 64 KiB) memory: about 1.3 MB for 10 GB. Lookups
/// binary-search the blocks, then count within one block of the source. The
/// index can be built in the background and answers for the scanned prefix
/// while it grows.
pub struct LineIndex {
    len: u64,
    state: RwLock<State>,
}

#[derive(Default)]
struct State {
    /// `newlines_before[i]` counts the newlines in `0..i * INDEX_BLOCK`.
    newlines_before: Vec<u64>,
    /// Bytes scanned so far.
    indexed: u64,
    /// Newlines in `0..indexed`.
    newlines: u64,
    /// Whether the last indexed byte is a newline.
    ends_with_newline: bool,
    failed: Option<io::ErrorKind>,
}

/// How far indexing has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub indexed: u64,
    pub len: u64,
    pub failed: Option<io::ErrorKind>,
}

impl Progress {
    pub fn is_complete(&self) -> bool {
        self.indexed == self.len
    }
}

impl LineIndex {
    fn new(len: u64) -> Self {
        Self {
            len,
            state: RwLock::new(State {
                newlines_before: vec![0],
                ..State::default()
            }),
        }
    }

    /// Indexes `source`: right away if it's in memory, otherwise on a
    /// background thread (see [`Self::spawn`]).
    pub fn for_source(source: Arc<FileSource>) -> io::Result<Arc<Self>> {
        if source.is_in_memory() {
            Ok(Arc::new(Self::build(&source)?))
        } else {
            Ok(Self::spawn(source))
        }
    }

    /// Indexes `source` on the calling thread.
    pub fn build(source: &FileSource) -> io::Result<Self> {
        let index = Self::new(source.len());
        let mut pending = Vec::new();
        source.scan(0..source.len(), |chunk| {
            pending.extend_from_slice(chunk);
            index.absorb(&mut pending, false);
            ControlFlow::Continue(())
        })?;
        index.absorb(&mut pending, true);
        Ok(index)
    }

    /// Indexes `source` on a background thread. The index fills in while the
    /// thread runs; dropping the index stops it.
    pub fn spawn(source: Arc<FileSource>) -> Arc<Self> {
        let index = Arc::new(Self::new(source.len()));
        let weak = Arc::downgrade(&index);
        let spawned = thread::Builder::new()
            .name("plastext-line-index".into())
            .spawn(move || Self::run(&source, &weak));
        if let Err(err) = spawned {
            index.state.write().unwrap().failed = Some(err.kind());
        }
        index
    }

    fn run(source: &FileSource, index: &Weak<Self>) {
        let mut pending = Vec::new();
        let result = source.scan(0..source.len(), |chunk| {
            let Some(index) = index.upgrade() else {
                return ControlFlow::Break(());
            };
            pending.extend_from_slice(chunk);
            index.absorb(&mut pending, false);
            ControlFlow::Continue(())
        });
        let Some(index) = index.upgrade() else { return };
        match result {
            Ok(()) => index.absorb(&mut pending, true),
            Err(err) => index.state.write().unwrap().failed = Some(err.kind()),
        }
    }

    /// Counts the whole blocks in `pending` (or all of it, at the end) and drops them.
    fn absorb(&self, pending: &mut Vec<u8>, end: bool) {
        let whole = pending.len() - pending.len() % INDEX_BLOCK as usize;
        let take = if end { pending.len() } else { whole };
        if take == 0 && !end {
            return;
        }
        let mut state = self.state.write().unwrap();
        if let Some(&last) = pending[..take].last() {
            state.ends_with_newline = last == b'\n';
        }
        for block in pending[..take].chunks(INDEX_BLOCK as usize) {
            state.newlines += memchr_iter(b'\n', block).count() as u64;
            state.indexed += block.len() as u64;
            if block.len() == INDEX_BLOCK as usize {
                let newlines = state.newlines;
                state.newlines_before.push(newlines);
            }
        }
        drop(state);
        pending.drain(..take);
    }

    pub fn progress(&self) -> Progress {
        let state = self.state.read().unwrap();
        Progress {
            indexed: state.indexed,
            len: self.len,
            failed: state.failed,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.progress().is_complete()
    }

    /// The number of lines, once indexing is complete. A trailing newline
    /// doesn't start another line, and an empty file has one line.
    pub fn line_count(&self) -> Option<u64> {
        let state = self.state.read().unwrap();
        (state.indexed == self.len).then(|| state.newlines + 1 - u64::from(state.ends_with_newline))
    }

    /// The number of lines, extrapolated from the indexed part while indexing runs.
    pub fn estimated_line_count(&self) -> u64 {
        let state = self.state.read().unwrap();
        if state.indexed == 0 {
            return 1;
        }
        let estimate = state.newlines as f64 * self.len as f64 / state.indexed as f64;
        estimate as u64 + 1
    }

    /// The 0-based line containing `offset`, or `None` while it isn't indexed.
    pub fn line_at(&self, offset: u64, source: &impl ByteSource) -> Result<Option<u64>, ReadError> {
        let (block_start, newlines_before) = {
            let state = self.state.read().unwrap();
            if offset > state.indexed || (offset == state.indexed && state.indexed < self.len) {
                return Ok(None);
            }
            let block = (offset / INDEX_BLOCK) as usize;
            let Some(&newlines_before) = state.newlines_before.get(block) else {
                return Ok(None);
            };
            (block as u64 * INDEX_BLOCK, newlines_before)
        };
        let bytes = source.bytes(block_start..offset)?;
        Ok(Some(
            newlines_before + memchr_iter(b'\n', &bytes).count() as u64,
        ))
    }

    /// The offset where 0-based `line` starts, or `None` if it's past the
    /// end or not indexed yet.
    pub fn line_start(
        &self,
        line: u64,
        source: &impl ByteSource,
    ) -> Result<Option<u64>, ReadError> {
        if line == 0 {
            return Ok(Some(0));
        }
        // Line `line` starts after the `line`-th newline.
        let (block, newlines_before) = {
            let state = self.state.read().unwrap();
            if line > state.newlines {
                return Ok(None);
            }
            let block = state.newlines_before.partition_point(|&n| n < line) - 1;
            (block as u64, state.newlines_before[block])
        };
        let block_start = block * INDEX_BLOCK;
        let block_end = (block_start + INDEX_BLOCK).min(self.len);
        let bytes = source.bytes(block_start..block_end)?;
        let nth = (line - newlines_before - 1) as usize;
        let newline = memchr_iter(b'\n', &bytes).nth(nth);
        let start = newline.map(|newline| block_start + newline as u64 + 1);
        Ok(start.filter(|&start| start < self.len))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Lines of pseudo-random lengths, some empty and some spanning index blocks.
    fn text(len: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..len)
            .map(|_| match next() % 100 {
                0 => b'\n',
                1..10 if next() % 50 == 0 => b'\n',
                _ => b'a' + (next() % 26) as u8,
            })
            .collect()
    }

    fn naive_line_starts(text: &[u8]) -> Vec<u64> {
        let mut starts = vec![0];
        starts.extend(
            memchr_iter(b'\n', text)
                .map(|i| i as u64 + 1)
                .filter(|&start| start < text.len() as u64),
        );
        starts
    }

    #[test]
    fn maps_lines_to_offsets_and_back() {
        for (len, seed) in [
            (0, 1),
            (1, 2),
            (5 * INDEX_BLOCK as usize + 123, 3),
            (2 * INDEX_BLOCK as usize, 4),
        ] {
            let contents = text(len, seed);
            let source = FileSource::from_bytes(contents.clone());
            let index = LineIndex::build(&source).unwrap();
            let starts = naive_line_starts(&contents);

            assert!(index.is_complete());
            assert_eq!(index.line_count(), Some(starts.len() as u64), "len {len}");
            for (line, &start) in starts.iter().enumerate() {
                assert_eq!(index.line_start(line as u64, &source), Ok(Some(start)));
                assert_eq!(index.line_at(start, &source), Ok(Some(line as u64)));
            }
            assert_eq!(index.line_start(starts.len() as u64 + 1, &source), Ok(None));
        }
    }

    #[test]
    fn background_indexing_matches_building_in_place() {
        let contents = text(40 * INDEX_BLOCK as usize + 17, 9);
        let source = Arc::new(FileSource::from_bytes(contents.clone()));
        let index = LineIndex::spawn(source.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !index.is_complete() {
            assert!(Instant::now() < deadline, "indexing didn't finish");
            thread::sleep(Duration::from_millis(1));
        }
        let built = LineIndex::build(&source).unwrap();
        assert_eq!(index.line_count(), built.line_count());
        assert_eq!(index.progress().failed, None);
    }

    #[test]
    fn unindexed_offsets_are_unknown() {
        let source = FileSource::from_bytes(text(3 * INDEX_BLOCK as usize, 5));
        let index = LineIndex::new(source.len());
        assert_eq!(index.line_at(10, &source), Ok(None));
        assert_eq!(index.line_start(3, &source), Ok(None));
        assert_eq!(index.line_count(), None);
        assert_eq!(index.estimated_line_count(), 1);
    }
}
