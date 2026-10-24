use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read as _},
    ops::{ControlFlow, Range},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use bytes::{Bytes, BytesMut};

/// Large files are read from disk and cached in blocks of this size (spec TXT-2).
pub const BLOCK_SIZE: u64 = 256 * 1024;

/// Read size for sequential scans, which bypass the block cache.
const SCAN_CHUNK: usize = 1024 * 1024;

/// How [`FileSource::open`] treats a file.
#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    /// Files up to this size are read into memory; larger ones are read in blocks on demand.
    pub large_file_threshold: u64,
    /// Memory budget for the cached blocks of one large file.
    pub block_cache: u64,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            large_file_threshold: 64 * 1024 * 1024,
            block_cache: 256 * 1024 * 1024,
        }
    }
}

/// Why bytes can't be returned without blocking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// This range isn't cached: [`FileSource::load`] it off the main thread, then retry.
    Missing(Range<u64>),
    /// The file shrank on disk after it was opened.
    Changed,
}

/// Read access to a document's bytes that never blocks.
pub trait ByteSource {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Exactly the bytes in `range`, which must lie within `0..len()`.
    fn bytes(&self, range: Range<u64>) -> Result<Bytes, ReadError>;
}

/// The contents of a file as opened, readable without blocking once loaded.
///
/// Small files are read whole. Large ones are read in [`BLOCK_SIZE`] blocks
/// with positional reads into a bounded LRU cache, never memory-mapped, so a
/// file truncated by another process can't crash us (spec TXT-2).
pub struct FileSource {
    len: u64,
    storage: Storage,
}

enum Storage {
    Memory(Bytes),
    Blocks(BlockFile),
}

struct BlockFile {
    file: File,
    capacity: usize,
    cache: Mutex<BlockCache>,
    changed: AtomicBool,
}

#[derive(Default)]
struct BlockCache {
    blocks: HashMap<u64, CachedBlock>,
    clock: u64,
}

struct CachedBlock {
    bytes: Bytes,
    last_used: u64,
}

impl FileSource {
    /// Opens `path`, reading it whole if it's small. Blocks: call it off the main thread.
    pub fn open(path: &Path, options: OpenOptions) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len <= options.large_file_threshold {
            let mut contents = Vec::with_capacity(len as usize);
            file.take(len).read_to_end(&mut contents)?;
            return Ok(Self::from_bytes(contents));
        }
        let capacity = (options.block_cache / BLOCK_SIZE).max(4) as usize;
        Ok(Self {
            len,
            storage: Storage::Blocks(BlockFile {
                file,
                capacity,
                cache: Mutex::default(),
                changed: AtomicBool::new(false),
            }),
        })
    }

    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        let bytes = bytes.into();
        Self {
            len: bytes.len() as u64,
            storage: Storage::Memory(bytes),
        }
    }

    pub fn is_in_memory(&self) -> bool {
        matches!(self.storage, Storage::Memory(_))
    }

    /// The whole contents, if the file was read into memory.
    pub fn in_memory(&self) -> Option<&Bytes> {
        match &self.storage {
            Storage::Memory(bytes) => Some(bytes),
            Storage::Blocks(_) => None,
        }
    }

    /// The part of `range` that isn't cached yet, if any. Never blocks.
    pub fn missing(&self, range: Range<u64>) -> Option<Range<u64>> {
        let Storage::Blocks(file) = &self.storage else {
            return None;
        };
        let range = self.clamp(range);
        if range.is_empty() {
            return None;
        }
        let cache = file.cache.lock().unwrap();
        let mut missing = block_indices(&range).filter(|index| !cache.blocks.contains_key(index));
        let first = missing.next()?;
        let last = missing.next_back().unwrap_or(first);
        Some(first * BLOCK_SIZE..((last + 1) * BLOCK_SIZE).min(self.len))
    }

    /// Caches every block overlapping `range`. Blocks: call it off the main thread.
    pub fn load(&self, range: Range<u64>) -> io::Result<()> {
        let Storage::Blocks(file) = &self.storage else {
            return Ok(());
        };
        let range = self.clamp(range);
        for index in block_indices(&range) {
            if file.cache.lock().unwrap().blocks.contains_key(&index) {
                continue;
            }
            let start = index * BLOCK_SIZE;
            let mut block = vec![0; (BLOCK_SIZE.min(self.len - start)) as usize];
            if let Err(err) = read_exact_at(&file.file, &mut block, start) {
                if err.kind() == io::ErrorKind::UnexpectedEof {
                    file.changed.store(true, Ordering::Relaxed);
                }
                return Err(err);
            }
            file.cache
                .lock()
                .unwrap()
                .insert(index, block.into(), file.capacity);
        }
        Ok(())
    }

    /// Reads `range` front to back, passing consecutive chunks to `f`, without
    /// filling the cache. Blocks: call it off the main thread.
    pub fn scan(
        &self,
        range: Range<u64>,
        mut f: impl FnMut(&[u8]) -> ControlFlow<()>,
    ) -> io::Result<()> {
        let range = self.clamp(range);
        match &self.storage {
            Storage::Memory(bytes) => {
                for chunk in bytes[range.start as usize..range.end as usize].chunks(SCAN_CHUNK) {
                    if f(chunk).is_break() {
                        break;
                    }
                }
            }
            Storage::Blocks(file) => {
                let mut buffer = vec![0; SCAN_CHUNK];
                let mut offset = range.start;
                while offset < range.end {
                    let chunk = &mut buffer[..(range.end - offset).min(SCAN_CHUNK as u64) as usize];
                    read_exact_at(&file.file, chunk, offset)?;
                    offset += chunk.len() as u64;
                    if f(chunk).is_break() {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn clamp(&self, range: Range<u64>) -> Range<u64> {
        range.start.min(self.len)..range.end.min(self.len)
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn bytes(&self, range: Range<u64>) -> Result<Bytes, ReadError> {
        debug_assert!(
            range.end <= self.len,
            "{range:?} is past the end ({})",
            self.len
        );
        let range = self.clamp(range);
        let file = match &self.storage {
            Storage::Memory(bytes) => {
                return Ok(bytes.slice(range.start as usize..range.end as usize));
            }
            Storage::Blocks(file) => file,
        };
        if file.changed.load(Ordering::Relaxed) {
            return Err(ReadError::Changed);
        }
        if range.is_empty() {
            return Ok(Bytes::new());
        }

        let mut cache = file.cache.lock().unwrap();
        cache.clock += 1;
        let clock = cache.clock;
        let mut blocks = Vec::new();
        for index in block_indices(&range) {
            match cache.blocks.get_mut(&index) {
                Some(block) => {
                    block.last_used = clock;
                    blocks.push((index * BLOCK_SIZE, block.bytes.clone()));
                }
                None => {
                    drop(cache);
                    let missing = self.missing(range.clone()).unwrap_or(range);
                    return Err(ReadError::Missing(missing));
                }
            }
        }
        drop(cache);

        let slice = |(block_start, block): &(u64, Bytes)| {
            let start = range.start.saturating_sub(*block_start) as usize;
            let end = ((range.end - block_start) as usize).min(block.len());
            block.slice(start..end)
        };
        if let [block] = blocks.as_slice() {
            return Ok(slice(block));
        }
        let mut joined = BytesMut::with_capacity((range.end - range.start) as usize);
        for block in &blocks {
            joined.extend_from_slice(&slice(block));
        }
        Ok(joined.freeze())
    }
}

impl BlockCache {
    fn insert(&mut self, index: u64, bytes: Bytes, capacity: usize) {
        while self.blocks.len() >= capacity {
            let oldest = self.blocks.iter().min_by_key(|(_, block)| block.last_used);
            let Some((&oldest, _)) = oldest else { break };
            self.blocks.remove(&oldest);
        }
        self.clock += 1;
        let last_used = self.clock;
        self.blocks.insert(index, CachedBlock { bytes, last_used });
    }
}

fn block_indices(range: &Range<u64>) -> Range<u64> {
    if range.is_empty() {
        return 0..0;
    }
    range.start / BLOCK_SIZE..(range.end - 1) / BLOCK_SIZE + 1
}

/// Fills `buffer` from `offset`, failing with `UnexpectedEof` if the file is shorter.
fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buffer.is_empty() {
        match read_at(file, buffer, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the file shrank after it was opened",
                ));
            }
            Ok(n) => {
                buffer = &mut buffer[n..];
                offset += n as u64;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn large_file(len: usize) -> (tempfile::NamedTempFile, Vec<u8>) {
        let contents: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&contents).unwrap();
        (file, contents)
    }

    fn open_in_blocks(path: &Path, block_cache: u64) -> FileSource {
        let options = OpenOptions {
            large_file_threshold: 0,
            block_cache,
        };
        FileSource::open(path, options).unwrap()
    }

    #[test]
    fn small_files_are_read_whole() {
        let (file, contents) = large_file(1000);
        let source = FileSource::open(file.path(), OpenOptions::default()).unwrap();
        assert!(source.is_in_memory());
        assert_eq!(source.missing(0..1000), None);
        assert_eq!(source.bytes(10..20).unwrap(), &contents[10..20]);
    }

    #[test]
    fn large_files_load_blocks_on_demand() {
        let len = 3 * BLOCK_SIZE as usize + 100;
        let (file, contents) = large_file(len);
        let source = open_in_blocks(file.path(), 64 * BLOCK_SIZE);
        assert!(!source.is_in_memory());

        let range = BLOCK_SIZE - 10..2 * BLOCK_SIZE + 10;
        assert_eq!(
            source.bytes(range.clone()),
            Err(ReadError::Missing(0..3 * BLOCK_SIZE))
        );
        source.load(range.clone()).unwrap();
        assert_eq!(source.missing(range.clone()), None);
        let bytes = source.bytes(range.clone()).unwrap();
        assert_eq!(bytes, &contents[range.start as usize..range.end as usize]);

        let tail = len as u64 - 50..len as u64;
        assert_eq!(
            source.missing(tail.clone()),
            Some(3 * BLOCK_SIZE..len as u64)
        );
        source.load(tail.clone()).unwrap();
        assert_eq!(source.bytes(tail).unwrap(), &contents[len - 50..]);
    }

    #[test]
    fn cache_evicts_least_recently_used_blocks() {
        let (file, _) = large_file(6 * BLOCK_SIZE as usize);
        let source = open_in_blocks(file.path(), 4 * BLOCK_SIZE);
        let block = |index: u64| index * BLOCK_SIZE..index * BLOCK_SIZE + 1;
        for index in 0..4 {
            source.load(block(index)).unwrap();
        }
        source.bytes(block(0)).unwrap();
        source.load(block(4)).unwrap();
        assert!(
            source.missing(block(1)).is_some(),
            "block 1 was least recently used"
        );
        assert_eq!(source.missing(block(0)), None);
        assert_eq!(source.missing(block(4)), None);
    }

    #[test]
    fn truncation_is_reported_not_fatal() {
        let (file, _) = large_file(2 * BLOCK_SIZE as usize);
        let source = open_in_blocks(file.path(), 64 * BLOCK_SIZE);
        file.as_file().set_len(10).unwrap();
        assert!(source.load(0..2 * BLOCK_SIZE).is_err());
        assert_eq!(source.bytes(0..5), Err(ReadError::Changed));
    }

    #[test]
    fn scan_reads_everything_in_order() {
        let (file, contents) = large_file(2 * SCAN_CHUNK + 7);
        for source in [
            open_in_blocks(file.path(), 64 * BLOCK_SIZE),
            FileSource::from_bytes(contents.clone()),
        ] {
            let mut scanned = Vec::new();
            source
                .scan(5..contents.len() as u64, |chunk| {
                    scanned.extend_from_slice(chunk);
                    ControlFlow::Continue(())
                })
                .unwrap();
            assert_eq!(scanned, &contents[5..]);
        }
    }
}
