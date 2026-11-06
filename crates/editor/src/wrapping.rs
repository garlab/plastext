//! Soft rows over the reader's bounded chunks. Never index a whole file or a
//! whole pathological line: work and retained boundaries stay viewport-local.
use crate::{folding::Folds, view_source::ViewSource};
use plastext_buffer::{ByteSource, ReadError};
use std::{cell::RefCell, collections::VecDeque, ops::Range, sync::Arc};

type WrappedChunk = (Range<u64>, Arc<[u32]>);

#[derive(Default)]
pub(crate) struct Wrapping {
    columns: Option<usize>,
    cache: RefCell<VecDeque<WrappedChunk>>,
}

impl Wrapping {
    pub fn configure(&mut self, columns: Option<usize>) -> bool {
        if self.columns == columns {
            return false;
        }
        self.columns = columns;
        self.clear();
        true
    }
    pub fn clear(&mut self) {
        self.cache.get_mut().clear();
    }
    pub fn columns(&self) -> Option<usize> {
        self.columns
    }
    pub fn enabled(&self) -> bool {
        self.columns.is_some()
    }
    fn breaks(
        &self,
        source: &ViewSource,
        offset: u64,
        tabs: usize,
    ) -> Result<WrappedChunk, ReadError> {
        // Find cached chunks before asking the paged reader to look backwards.
        if let Some(found) = self
            .cache
            .borrow()
            .iter()
            .find(|(range, _)| {
                range.contains(&offset) || (range.end == source.len() && offset == range.end)
            })
            .cloned()
        {
            return Ok(found);
        }
        let start = source.row_start(offset)?;
        let range = start..source.row_end(start)?;
        let bytes = source.bytes(range.clone())?;
        let breaks: Arc<[u32]> = split(&bytes, self.columns.unwrap(), tabs).into();
        let result = (range, breaks);
        let mut cache = self.cache.borrow_mut();
        // At most eight 64 KiB chunks, even at a one-column viewport width.
        if cache.len() == 8 {
            cache.pop_front();
        }
        cache.push_back(result.clone());
        Ok(result)
    }
    pub fn start(&self, source: &ViewSource, offset: u64, tabs: usize) -> Result<u64, ReadError> {
        if !self.enabled() {
            return source.row_start(offset);
        }
        if offset >= source.len()
            && source.snapshot.is_some()
            && source.starts_line(source.len())?
        {
            return Ok(source.len());
        }
        let (range, breaks) = self.breaks(source, offset, tabs)?;
        let i = breaks.partition_point(|&end| range.start + end as u64 <= offset);
        // EOF belongs to the last row, unless the snapshot has an empty final line.
        let i = i.min(breaks.len().saturating_sub(1));
        Ok(range.start + i.checked_sub(1).map_or(0, |i| breaks[i] as u64))
    }
    pub fn end(&self, source: &ViewSource, start: u64, tabs: usize) -> Result<u64, ReadError> {
        if !self.enabled() {
            return source.row_end(start);
        }
        if start == source.len() {
            return Ok(start);
        }
        let (range, breaks) = self.breaks(source, start, tabs)?;
        let i = breaks.partition_point(|&end| range.start + end as u64 <= start);
        Ok(range.start + breaks[i] as u64)
    }
    pub fn next(
        &self,
        source: &ViewSource,
        folds: &Folds,
        start: u64,
        tabs: usize,
    ) -> Result<Option<u64>, ReadError> {
        if !self.enabled() {
            return source.next(folds, start);
        }
        let end = self.end(source, start, tabs)?;
        if let Some(hidden) = folds.hidden_at(end) {
            return Ok((hidden.end < source.len()).then_some(hidden.end));
        }
        Ok((end > start
            && (end < source.len() || (source.snapshot.is_some() && source.starts_line(end)?)))
        .then_some(end))
    }
    pub fn previous(
        &self,
        source: &ViewSource,
        folds: &Folds,
        start: u64,
        tabs: usize,
    ) -> Result<Option<u64>, ReadError> {
        if !self.enabled() {
            return source.previous(folds, start);
        }
        if start == 0 {
            return Ok(None);
        }
        let offset = folds
            .hidden_at(start - 1)
            .map_or(start - 1, |range| range.start - 1);
        self.start(source, offset, tabs).map(Some)
    }
    pub fn visible(
        &self,
        source: &ViewSource,
        folds: &Folds,
        start: u64,
        count: usize,
        tabs: usize,
    ) -> (Vec<Range<u64>>, Option<ReadError>) {
        if !self.enabled() {
            return source.visible_rows(folds, start, count);
        }
        let mut ranges = Vec::with_capacity(count);
        let mut row = start;
        for _ in 0..count {
            match self.end(source, row, tabs) {
                Ok(end) => ranges.push(row..end),
                Err(error) => return (ranges, Some(error)),
            }
            match self.next(source, folds, row, tabs) {
                Ok(Some(next)) => row = next,
                Ok(None) => break,
                Err(error) => return (ranges, Some(error)),
            }
        }
        (ranges, None)
    }
}

/// Prefer whitespace boundaries, retaining every source byte for copying and
/// editing. Long words break at scalar boundaries; tabs use visual-row stops.
fn split(bytes: &[u8], columns: usize, tabs: usize) -> Vec<u32> {
    let content = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let content = content.strip_suffix(b"\r").unwrap_or(content);
    let text = String::from_utf8_lossy(content);
    // Lossy conversion would change byte coordinates. Invalid UTF-8 is rare in
    // a source view; one byte per replacement keeps its offsets intact.
    let chars: Vec<(usize, usize, char)> = if text.len() == content.len() {
        text.char_indices()
            .map(|(i, ch)| (i, ch.len_utf8(), ch))
            .collect()
    } else {
        let mut offset = 0;
        content
            .utf8_chunks()
            .flat_map(|chunk| {
                let start = offset;
                offset += chunk.valid().len() + chunk.invalid().len();
                chunk
                    .valid()
                    .char_indices()
                    .map(move |(i, ch)| (start + i, ch.len_utf8(), ch))
                    .chain((!chunk.invalid().is_empty()).then_some((
                        start + chunk.valid().len(),
                        chunk.invalid().len(),
                        '\u{fffd}',
                    )))
            })
            .collect()
    };
    let mut ends = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut width = 0;
        let mut at = start;
        let mut word_break = None;
        while at < chars.len() {
            let ch = chars[at].2;
            let advance = if ch == '\t' {
                tabs.max(1) - width % tabs.max(1)
            } else {
                1
            };
            if width + advance > columns.max(1) && at > start {
                break;
            }
            width += advance;
            at += 1;
            if ch.is_whitespace() {
                word_break = Some(at);
            }
        }
        if at == chars.len() {
            break;
        }
        let end = word_break.unwrap_or(at);
        ends.push(chars[end].0 as u32);
        start = end;
    }
    ends.push(bytes.len() as u32);
    ends
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_buffer::{EditBuffer, FileSource};

    #[test]
    fn words_tabs_unicode_and_newlines_keep_their_source_offsets() {
        assert_eq!(split(b"one two three\r\n", 8, 4), [8, 15]);
        assert_eq!(split("ééé 😀 x\n".as_bytes(), 4, 4), [7, 14]);
        assert_eq!(split(b"a\tbcd\n", 4, 4), [2, 6]);
        assert_eq!(split(b"\xff\xfeabcdef", 2, 4), [2, 4, 6, 8]);
        let text = "one two three\r\n\nαβ γδ εζ\rfinal line\n"
            .as_bytes()
            .to_vec();
        let source = ViewSource {
            original: Arc::new(FileSource::from_bytes(Vec::new())),
            snapshot: Some(EditBuffer::from_bytes(text.clone()).snapshot()),
        };
        let mut wrapping = Wrapping::default();
        for columns in [1, 4, 8, 200] {
            wrapping.configure(Some(columns));
            let folds = Folds::default();
            let (rows, error) = wrapping.visible(&source, &folds, 0, 1000, 4);
            assert_eq!(error, None);
            assert_eq!(rows.last(), Some(&(text.len() as u64..text.len() as u64)));
            for (i, row) in rows.iter().enumerate() {
                assert!(std::str::from_utf8(&text[row.start as usize..row.end as usize]).is_ok());
                assert_eq!(
                    wrapping.previous(&source, &folds, row.start, 4).unwrap(),
                    i.checked_sub(1).map(|i| rows[i].start)
                );
                for offset in row.clone() {
                    // Byte offsets inside UTF-8 and CRLF still map to this row.
                    assert_eq!(wrapping.start(&source, offset, 4).unwrap(), row.start);
                }
                if let Some(next) = rows.get(i + 1) {
                    assert_eq!(row.end, next.start);
                }
            }
        }
    }

    #[test]
    fn huge_line_only_loads_near_the_requested_position() {
        use plastext_buffer::OpenOptions;
        // Sparse data exercises the real paged reader without a 500 MB allocation.
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(500 * 1024 * 1024).unwrap();
        let source = ViewSource {
            original: Arc::new(
                FileSource::open(
                    file.path(),
                    OpenOptions {
                        large_file_threshold: 0,
                        block_cache: 1024 * 1024,
                    },
                )
                .unwrap(),
            ),
            snapshot: None,
        };
        let middle = source.len() / 2;
        source
            .load(middle - 256 * 1024..middle + 256 * 1024)
            .unwrap();
        let mut wrapping = Wrapping::default();
        wrapping.configure(Some(80));
        let start = wrapping.start(&source, middle, 4).unwrap();
        assert!(middle - start < 80);
        let (rows, error) = wrapping.visible(&source, &Folds::default(), start, 60, 4);
        assert_eq!(error, None);
        assert_eq!(rows.len(), 60);
        assert!(rows.iter().all(|row| row.end - row.start <= 80));
        assert!(
            source.bytes(0..80).is_err(),
            "wrapping must not read from the line's beginning"
        );
        assert!(wrapping.cache.borrow().len() <= 8);
    }

    #[test]
    fn wrapping_preserves_fold_boundaries() {
        let source = ViewSource {
            original: Arc::new(FileSource::from_bytes(
                b"long header words\n  hidden\nlast\n".to_vec(),
            )),
            snapshot: None,
        };
        let mut folds = Folds::default();
        folds.fold(18..27);
        let mut wrapping = Wrapping::default();
        wrapping.configure(Some(8));
        let (rows, error) = wrapping.visible(&source, &folds, 0, 20, 4);
        assert_eq!(error, None);
        assert_eq!(rows, [0..5, 5..12, 12..18, 27..32]);
        assert_eq!(wrapping.previous(&source, &folds, 27, 4), Ok(Some(12)));
    }
}
