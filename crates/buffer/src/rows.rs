//! Display rows: the lines of a text, with very long lines split so that no
//! row exceeds [`MAX_ROW_LEN`] bytes (spec TXT-6).
//!
//! A row boundary depends only on nearby bytes, so rows can be found from any
//! offset without scanning from the start of the line, which may be hundreds
//! of megabytes back. Each row includes its trailing newline, and a line
//! shorter than [`SPLIT`] bytes is always a single row. In a longer line, a
//! multiple of [`SPLIT`] (moved forward to the next UTF-8 character boundary)
//! is a split point when the [`SPLIT`] bytes before it hold no newline. Long
//! lines therefore break into rows of about [`SPLIT`] bytes at fixed offsets.

use std::ops::Range;

use memchr::{memchr, memrchr};

use crate::{ByteSource, ReadError};

/// Lines longer than this may be split into several rows.
pub const SPLIT: u64 = 32 * 1024;

/// No row is longer than this.
pub const MAX_ROW_LEN: u64 = 2 * SPLIT + 3;

/// The end of the row starting at `start`, which must be a row start.
pub fn row_end(source: &impl ByteSource, start: u64) -> Result<u64, ReadError> {
    let len = source.len();
    if start >= len {
        return Ok(len);
    }
    let window_end = len.min(start + MAX_ROW_LEN + 1);
    let window = source.bytes(start..window_end)?;
    let newline = memchr(b'\n', &window).map(|i| start + i as u64);
    // Where this line's content ends, as far as the window shows.
    let content_end = newline.unwrap_or(window_end);

    let first = (start / SPLIT + 1) * SPLIT;
    for candidate in [first, first + SPLIT] {
        if candidate >= content_end {
            break;
        }
        // The window shows no newline in `start..candidate`, but the first
        // candidate's lookback can reach before `start`.
        if candidate - SPLIT < start {
            let before = source.bytes(candidate - SPLIT..start)?;
            if memchr(b'\n', &before).is_some() {
                continue;
            }
        }
        let split = start + char_boundary(&window, (candidate - start) as usize) as u64;
        if split < content_end {
            return Ok(split);
        }
        break;
    }
    Ok(newline.map_or(window_end, |newline| newline + 1))
}

/// The start of the row containing `offset`. Offsets past the end map to the last row.
pub fn row_start_at(source: &impl ByteSource, offset: u64) -> Result<u64, ReadError> {
    let len = source.len();
    if len == 0 {
        return Ok(0);
    }
    let offset = offset.min(len - 1);
    let lookback = offset.saturating_sub(3 * SPLIT + 8)..offset;
    let before = source.bytes(lookback.clone())?;
    let mut row = match memrchr(b'\n', &before) {
        Some(newline) => lookback.start + newline as u64 + 1,
        None if lookback.start == 0 => 0,
        None => {
            // No newline anywhere in the lookback, so its multiples of SPLIT
            // with a full SPLIT of lookback before them are all split points.
            let candidate = (lookback.start / SPLIT + 2) * SPLIT;
            let at = (candidate - lookback.start) as usize;
            candidate + (char_boundary(&before, at) - at) as u64
        }
    };
    loop {
        let end = row_end(source, row)?;
        if end > offset || end >= len {
            return Ok(row);
        }
        row = end;
    }
}

/// The start of the row before the one starting at `start`, if any.
pub fn previous_row_start(source: &impl ByteSource, start: u64) -> Result<Option<u64>, ReadError> {
    if start == 0 {
        return Ok(None);
    }
    row_start_at(source, start - 1).map(Some)
}

/// Whether the row starting at `start` begins a line rather than continuing a split one.
pub fn starts_line(source: &impl ByteSource, start: u64) -> Result<bool, ReadError> {
    if start == 0 {
        return Ok(true);
    }
    Ok(source.bytes(start - 1..start)?[0] == b'\n')
}

/// Up to `count` consecutive rows from the row start `start`.
///
/// Stops early at the end of the text or at the first unavailable range,
/// which it returns alongside the rows found before it.
pub fn rows(
    source: &impl ByteSource,
    start: u64,
    count: usize,
) -> (Vec<Range<u64>>, Option<ReadError>) {
    let mut rows = Vec::with_capacity(count.min(256));
    let mut row = start;
    while rows.len() < count && (row < source.len() || (row == 0 && rows.is_empty())) {
        match row_end(source, row) {
            Ok(end) => {
                rows.push(row..end);
                if end == row {
                    break;
                }
                row = end;
            }
            Err(err) => return (rows, Some(err)),
        }
    }
    (rows, None)
}

/// `at` moved past UTF-8 continuation bytes, by at most three.
fn char_boundary(bytes: &[u8], at: usize) -> usize {
    let limit = bytes.len().min(at + 3);
    let mut at = at;
    while at < limit && bytes[at] & 0xC0 == 0x80 {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileSource;

    fn all_rows(source: &FileSource) -> Vec<Range<u64>> {
        let (rows, err) = rows(source, 0, usize::MAX);
        assert_eq!(err, None);
        rows
    }

    /// A text mixing short lines, empty lines and lines several SPLITs long,
    /// some made of multibyte characters.
    fn mixed_text() -> Vec<u8> {
        let mut text = Vec::new();
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..60 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let line_len = match state % 5 {
                0 => 0,
                1 => (state >> 8) % 100,
                2 => (state >> 8) % (5 * SPLIT),
                _ => (state >> 8) % 300,
            };
            let multibyte = state.is_multiple_of(3);
            while (text.len() as u64) < line_len {
                if multibyte {
                    text.extend_from_slice("é€😀".as_bytes());
                } else {
                    text.push(b'x');
                }
            }
            text.push(b'\n');
        }
        text
    }

    #[test]
    fn short_lines_are_single_rows() {
        let source = FileSource::from_bytes("one\n\nthree\nfour");
        assert_eq!(all_rows(&source), [0..4, 4..5, 5..11, 11..15]);
        assert_eq!(row_start_at(&source, 7), Ok(5));
        assert_eq!(row_start_at(&source, 4), Ok(4));
        assert_eq!(row_start_at(&source, 100), Ok(11));
        assert_eq!(previous_row_start(&source, 5), Ok(Some(4)));
        assert_eq!(previous_row_start(&source, 0), Ok(None));
    }

    #[test]
    fn empty_text_has_one_empty_row() {
        let source = FileSource::from_bytes("");
        assert_eq!(all_rows(&source), vec![0..0]);
        assert_eq!(row_start_at(&source, 0), Ok(0));
    }

    #[test]
    fn long_lines_split_at_fixed_offsets() {
        let starts = |source: &FileSource| -> Vec<u64> {
            all_rows(source).iter().map(|row| row.start).collect()
        };

        // From the start of the text, the first SPLIT bytes hold no newline.
        let source = FileSource::from_bytes(vec![b'x'; 10 * SPLIT as usize + 5]);
        let expected: Vec<u64> = (0..=10).map(|i| i * SPLIT).collect();
        assert_eq!(starts(&source), expected);

        // After a newline, the first multiple of SPLIT still sees that newline.
        let mut text = vec![b'y'; 100];
        text.push(b'\n');
        text.extend(vec![b'x'; 10 * SPLIT as usize]);
        let source = FileSource::from_bytes(text);
        let expected: Vec<u64> = [0, 101]
            .into_iter()
            .chain((2..=10).map(|i| i * SPLIT))
            .collect();
        assert_eq!(starts(&source), expected);
        assert!(
            all_rows(&source)
                .iter()
                .all(|row| row.end - row.start <= MAX_ROW_LEN)
        );
        assert_eq!(starts_line(&source, 101), Ok(true));
        assert_eq!(starts_line(&source, 2 * SPLIT), Ok(false));
    }

    #[test]
    fn rows_tile_the_text_and_agree_with_row_start_at() {
        let text = mixed_text();
        let source = FileSource::from_bytes(text.clone());
        let rows = all_rows(&source);

        assert_eq!(rows.first().map(|row| row.start), Some(0));
        assert_eq!(rows.last().map(|row| row.end), Some(text.len() as u64));
        for pair in rows.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for row in &rows {
            assert!(row.end - row.start <= MAX_ROW_LEN, "{row:?} is too long");
            let bytes = &text[row.start as usize..row.end as usize];
            assert!(
                std::str::from_utf8(bytes).is_ok(),
                "{row:?} splits a character"
            );
            let newline = bytes.iter().position(|&b| b == b'\n');
            assert!(newline.is_none() || newline == Some(bytes.len() - 1));
            if bytes.len() < SPLIT as usize && starts_line(&source, row.start).unwrap() {
                assert!(bytes.ends_with(b"\n") || row.end == text.len() as u64);
            }
        }
        for offset in (0..text.len() as u64).step_by(997) {
            let row = rows.iter().find(|row| row.contains(&offset)).unwrap();
            assert_eq!(
                row_start_at(&source, offset),
                Ok(row.start),
                "offset {offset}"
            );
        }
        for pair in rows.windows(2) {
            assert_eq!(
                previous_row_start(&source, pair[1].start),
                Ok(Some(pair[0].start))
            );
        }
    }

    #[test]
    fn unloaded_blocks_are_reported() {
        use std::io::Write as _;

        let text = mixed_text();
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&text).unwrap();
        let options = crate::OpenOptions {
            large_file_threshold: 0,
            ..Default::default()
        };
        let source = FileSource::open(file.path(), options).unwrap();

        let (rows, err) = rows(&source, 0, 10);
        assert!(rows.is_empty());
        assert!(matches!(err, Some(ReadError::Missing(range)) if range.start == 0));

        source.load(0..text.len() as u64).unwrap();
        let loaded = all_rows(&source);
        assert_eq!(loaded, all_rows(&FileSource::from_bytes(text)));
    }
}
