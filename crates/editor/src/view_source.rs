//! A viewport can keep its cheap reader until an editing snapshot is ready.
use crate::folding::{self, Folds};
use bytes::Bytes;
use plastext_buffer::{Bias, ByteSource, FileSource, LineIndex, ReadError, TextSnapshot, rows};
use std::{io, ops::Range, sync::Arc};

#[derive(Clone)]
pub(crate) struct ViewSource {
    pub original: Arc<FileSource>,
    pub snapshot: Option<TextSnapshot>,
}
impl ByteSource for ViewSource {
    fn len(&self) -> u64 {
        self.snapshot
            .as_ref()
            .map_or_else(|| self.original.len(), ByteSource::len)
    }
    fn bytes(&self, range: Range<u64>) -> Result<Bytes, ReadError> {
        match &self.snapshot {
            Some(text) => text.bytes(range),
            None => self.original.bytes(range),
        }
    }
}
impl ViewSource {
    pub fn in_memory(&self) -> Option<&Bytes> {
        if self.snapshot.is_some() {
            None
        } else {
            self.original.in_memory()
        }
    }
    pub fn load(&self, range: Range<u64>) -> io::Result<()> {
        let range = range.start.min(self.len())..range.end.min(self.len());
        match &self.snapshot {
            Some(text) => text.load(range),
            None => self.original.load(range),
        }
    }
    pub fn missing(&self, range: Range<u64>) -> Option<Range<u64>> {
        let range = range.start.min(self.len())..range.end.min(self.len());
        match &self.snapshot {
            Some(text) => text.missing(range),
            None => self.original.missing(range),
        }
    }
    pub fn line_at(&self, index: &LineIndex, offset: u64) -> Result<Option<u64>, ReadError> {
        match &self.snapshot {
            Some(text) => text.line_at(offset.min(text.len())).map(Some),
            None => index.line_at(offset, &*self.original),
        }
    }
    pub fn line_start(&self, index: &LineIndex, line: u64) -> Result<Option<u64>, ReadError> {
        match &self.snapshot {
            Some(text) => text.line_start(line),
            None => index.line_start(line, &*self.original),
        }
    }
    pub fn line_count(&self, index: &LineIndex) -> u64 {
        self.snapshot.as_ref().map_or_else(
            || {
                index
                    .line_count()
                    .unwrap_or_else(|| index.estimated_line_count())
            },
            TextSnapshot::line_count,
        )
    }
    pub fn row_start(&self, offset: u64) -> Result<u64, ReadError> {
        let Some(text) = &self.snapshot else {
            return rows::row_start_at(self, offset);
        };
        let offset = offset.min(text.len());
        let line = text
            .line_start(text.line_at(offset)?)
            .map(|value| value.unwrap_or(0))?;
        let candidate = line + (offset - line) / rows::SPLIT * rows::SPLIT;
        let clipped = self.boundary(candidate)?;
        if clipped > offset {
            self.boundary(candidate.saturating_sub(rows::SPLIT).max(line))
        } else {
            Ok(clipped)
        }
    }
    fn boundary(&self, offset: u64) -> Result<u64, ReadError> {
        let text = self.snapshot.as_ref().unwrap();
        let mut offset = text
            .clip_to_scalar(offset.min(text.len()), Bias::After)
            .map_err(|error| match error {
                plastext_buffer::TextError::Read(error) => error,
                _ => ReadError::Changed,
            })?;
        if offset > 0
            && offset < text.len()
            && text.bytes(offset - 1..offset + 1)?.as_ref() == b"\r\n"
        {
            offset += 1;
        }
        Ok(offset)
    }
    pub fn row_end(&self, start: u64) -> Result<u64, ReadError> {
        let Some(text) = &self.snapshot else {
            return rows::row_end(self, start);
        };
        if start >= text.len() {
            return Ok(text.len());
        }
        let line = text.line_at(start)?;
        let base = text.line_start(line)?.unwrap_or(0);
        let end = text.line_start(line + 1)?.unwrap_or(text.len());
        self.boundary((base + ((start - base) / rows::SPLIT + 1) * rows::SPLIT).min(end))
    }
    pub fn starts_line(&self, start: u64) -> Result<bool, ReadError> {
        match &self.snapshot {
            Some(text) => Ok(text.line_start(text.line_at(start)?)? == Some(start)),
            None => rows::starts_line(self, start),
        }
    }
    pub fn next(&self, folds: &Folds, row: u64) -> Result<Option<u64>, ReadError> {
        if self.snapshot.is_none() {
            return folding::next_visible_row(self, folds, row);
        }
        let end = self.row_end(row)?;
        Ok((end > row && (end < self.len() || self.starts_line(end)?)).then_some(end))
    }
    pub fn previous(&self, folds: &Folds, row: u64) -> Result<Option<u64>, ReadError> {
        if self.snapshot.is_none() {
            return folding::previous_visible_row(self, folds, row);
        }
        if row == 0 {
            Ok(None)
        } else {
            self.row_start(row - 1).map(Some)
        }
    }
    pub fn visible_rows(
        &self,
        folds: &Folds,
        start: u64,
        count: usize,
    ) -> (Vec<Range<u64>>, Option<ReadError>) {
        if self.snapshot.is_none() {
            return folding::visible_rows(self, folds, start, count);
        }
        let mut result = Vec::new();
        let mut row = start;
        for _ in 0..count {
            match self.row_end(row) {
                Ok(end) => result.push(row..end),
                Err(error) => return (result, Some(error)),
            }
            match self.next(folds, row) {
                Ok(Some(next)) => row = next,
                Ok(None) => break,
                Err(error) => return (result, Some(error)),
            }
        }
        (result, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_buffer::EditBuffer;

    fn source(bytes: Vec<u8>) -> ViewSource {
        ViewSource {
            original: Arc::new(FileSource::from_bytes(bytes.clone())),
            snapshot: Some(EditBuffer::from_bytes(bytes).snapshot()),
        }
    }
    #[test]
    fn mixed_newlines_and_empty_eof_are_rows() {
        for (bytes, expected) in [
            (b"a\r\nb\rc\n".to_vec(), vec![0..3, 3..5, 5..7, 7..7]),
            (vec![], std::iter::once(0..0).collect()),
            (b"abc".to_vec(), std::iter::once(0..3).collect()),
        ] {
            let source = source(bytes);
            let folds = Folds::default();
            let (rows, err) = source.visible_rows(&folds, 0, 100);
            assert_eq!(err, None);
            assert_eq!(rows, expected);
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(source.row_start(row.start).unwrap(), row.start);
                assert_eq!(
                    source.previous(&folds, row.start).unwrap(),
                    i.checked_sub(1).map(|i| rows[i].start)
                );
            }
        }
    }
    #[test]
    fn long_rows_never_split_scalars_or_crlf() {
        for padding in 0..4 {
            let mut bytes = vec![b'x'; rows::SPLIT as usize - padding];
            bytes.extend_from_slice("😀".as_bytes());
            bytes.extend(std::iter::repeat_n(b'y', rows::SPLIT as usize - 1));
            bytes.extend_from_slice(b"\r\nlast");
            let source = source(bytes.clone());
            let folds = Folds::default();
            let (rows, err) = source.visible_rows(&folds, 0, 100);
            assert_eq!(err, None);
            assert_eq!(rows.last().unwrap().end, bytes.len() as u64);
            for (i, row) in rows.iter().enumerate() {
                assert!(std::str::from_utf8(&bytes[row.start as usize..row.end as usize]).is_ok());
                assert_eq!(source.row_start(row.start).unwrap(), row.start);
                assert_eq!(
                    source.previous(&folds, row.start).unwrap(),
                    i.checked_sub(1).map(|i| rows[i].start)
                );
                assert!(
                    row.end == bytes.len() as u64
                        || bytes[row.end as usize - 1..=row.end as usize] != *b"\r\n"
                );
            }
        }
    }
}
