//! Sparse record indexing for CSV/TSV, including quoted multiline fields.
use plastext_buffer::{ByteSource as _, FileSource};
use std::{
    io,
    ops::{ControlFlow, Range},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
const STRIDE: u64 = 512;
const RECORD_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub struct CsvProgress {
    pub rows: u64,
    pub bytes: u64,
    pub complete: bool,
}
#[derive(Default)]
struct State {
    anchors: Vec<u64>,
    progress: CsvProgress,
    error: Option<String>,
}
pub struct CsvIndex {
    state: Mutex<State>,
}
impl CsvIndex {
    pub fn start(
        source: Arc<FileSource>,
        delimiter: u8,
        cancel: Arc<AtomicBool>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Arc<Self> {
        let index = Arc::new(Self {
            state: Mutex::new(State {
                anchors: vec![0],
                ..Default::default()
            }),
        });
        let output = index.clone();
        std::thread::spawn(move || {
            let mut scanner = Scanner::new(delimiter, 0);
            let mut count: u64 = 0;
            let mut anchors = Vec::new();
            let result = source.scan(0..source.len(), |chunk| {
                if cancel.load(Ordering::Relaxed) {
                    return ControlFlow::Break(());
                }
                scanner.feed(chunk, |range| {
                    count += 1;
                    if count.is_multiple_of(STRIDE) {
                        anchors.push(range.end);
                    }
                });
                let mut state = output.state.lock().unwrap();
                state.anchors.append(&mut anchors);
                state.progress = CsvProgress {
                    rows: count,
                    bytes: scanner.offset,
                    complete: false,
                };
                drop(state);
                notify();
                ControlFlow::Continue(())
            });
            if !cancel.load(Ordering::Relaxed) {
                if scanner.start < scanner.offset {
                    count += 1;
                }
                let mut state = output.state.lock().unwrap();
                state.progress = CsvProgress {
                    rows: count,
                    bytes: source.len(),
                    complete: true,
                };
                state.error = result.err().map(|e| e.to_string());
                drop(state);
                notify();
            }
        });
        index
    }
    pub fn progress(&self) -> CsvProgress {
        self.state.lock().unwrap().progress
    }
    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }
    pub fn page(
        &self,
        source: &FileSource,
        delimiter: u8,
        first: u64,
        count: usize,
    ) -> io::Result<Vec<CsvRow>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let block = first / STRIDE;
        let offset = {
            let state = self.state.lock().unwrap();
            *state
                .anchors
                .get(block as usize)
                .ok_or_else(|| io::Error::other("Rows are not indexed yet"))?
        };
        let mut scanner = Scanner::new(delimiter, offset);
        let mut row = block * STRIDE;
        let mut rows = Vec::new();
        let mut record = Vec::new();
        let mut record_start = offset;
        let mut oversized = false;
        source.scan(offset..source.len(), |chunk| {
            // Feed bytes individually so records crossing scan blocks remain bounded.
            for &byte in chunk {
                if row >= first {
                    if record.len() < RECORD_LIMIT {
                        record.push(byte);
                    } else {
                        oversized = true;
                    }
                }
                let mut ended = None;
                scanner.feed(std::slice::from_ref(&byte), |range| ended = Some(range));
                if let Some(range) = ended {
                    if row >= first {
                        rows.push(CsvRow {
                            source: record_start..range.end,
                            cells: decode_record(&record, delimiter, oversized),
                        });
                    }
                    record.clear();
                    oversized = false;
                    record_start = range.end;
                    row += 1;
                    if rows.len() >= count {
                        return ControlFlow::Break(());
                    }
                }
            }
            ControlFlow::Continue(())
        })?;
        if rows.len() < count && scanner.start < scanner.offset && row >= first {
            rows.push(CsvRow {
                source: record_start..scanner.offset,
                cells: decode_record(&record, delimiter, oversized),
            });
        }
        Ok(rows)
    }

    /// Locate the record containing a source byte, using the nearest sparse
    /// anchor. Quoted newlines do not change the record number.
    pub fn row_for_offset(
        &self,
        source: &FileSource,
        delimiter: u8,
        offset: u64,
    ) -> io::Result<u64> {
        let offset = offset.min(source.len().saturating_sub(1));
        let (block, start) = {
            let state = self.state.lock().unwrap();
            if !state.progress.complete && offset >= state.progress.bytes {
                return Err(io::Error::other("Position is not indexed yet"));
            }
            let block = state
                .anchors
                .partition_point(|&a| a <= offset)
                .saturating_sub(1);
            (block as u64, state.anchors[block])
        };
        let mut row = block * STRIDE;
        let mut scanner = Scanner::new(delimiter, start);
        source.scan(start..source.len(), |chunk| {
            for &byte in chunk {
                if scanner.offset > offset {
                    return ControlFlow::Break(());
                }
                scanner.feed(std::slice::from_ref(&byte), |range| {
                    if range.end <= offset {
                        row += 1;
                    }
                });
            }
            ControlFlow::Continue(())
        })?;
        Ok(row)
    }
}
#[derive(Clone, Debug)]
pub struct CsvRow {
    pub source: Range<u64>,
    pub cells: Result<Vec<String>, String>,
}
fn decode_record(record: &[u8], delimiter: u8, oversized: bool) -> Result<Vec<String>, String> {
    if oversized {
        return Err("Record exceeds the 64 KiB table preview limit; use Source.".into());
    }
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(record);
    match reader.byte_records().next() {
        Some(Ok(record)) if record.len() <= 128 => Ok(record
            .iter()
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect()),
        Some(Ok(_)) => Err("Record exceeds 128 columns; use Source.".into()),
        Some(Err(error)) => Err(error.to_string()),
        None => Ok(Vec::new()),
    }
}
#[derive(Clone, Copy)]
enum Quote {
    Start,
    Plain,
    Quoted,
    Closed,
}
struct Scanner {
    delimiter: u8,
    quote: Quote,
    start: u64,
    offset: u64,
}
impl Scanner {
    fn new(delimiter: u8, offset: u64) -> Self {
        Self {
            delimiter,
            quote: Quote::Start,
            start: offset,
            offset,
        }
    }
    fn feed(&mut self, bytes: &[u8], mut row: impl FnMut(Range<u64>)) {
        for &byte in bytes {
            self.offset += 1;
            match (self.quote, byte) {
                (Quote::Quoted, b'"') => self.quote = Quote::Closed,
                (Quote::Quoted, _) => {}
                (Quote::Closed, b'"') => self.quote = Quote::Quoted,
                (_, b'\n') => {
                    row(self.start..self.offset);
                    self.start = self.offset;
                    self.quote = Quote::Start;
                }
                (_, b) if b == self.delimiter => self.quote = Quote::Start,
                (Quote::Start, b'"') => self.quote = Quote::Quoted,
                _ => self.quote = Quote::Plain,
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_records_cross_chunks_and_sparse_anchors() {
        let text = format!(
            "a,b\r\n{}\"last\",tail",
            "\"line\nwith \"\"quotes\"\"\",ok\r\n".repeat(1200)
        );
        let source = Arc::new(FileSource::from_bytes(text.into_bytes()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let index = CsvIndex::start(
            source.clone(),
            b',',
            cancel,
            Arc::new(move || {
                tx.send(()).ok();
            }),
        );
        while !index.progress().complete {
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        }
        assert_eq!(index.progress().rows, 1202);
        let rows = index.page(&source, b',', 510, 8).unwrap();
        assert_eq!(rows.len(), 8);
        assert!(index.page(&source, b',', 0, 0).unwrap().is_empty());
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(
                index
                    .row_for_offset(&source, b',', row.source.start)
                    .unwrap(),
                510 + i as u64
            );
            assert_eq!(
                index
                    .row_for_offset(&source, b',', row.source.end - 1)
                    .unwrap(),
                510 + i as u64
            );
        }
        assert_eq!(
            rows[0].cells.as_ref().unwrap(),
            &["line\nwith \"quotes\"", "ok"]
        );
        assert_eq!(
            index.page(&source, b',', 1201, 1).unwrap()[0]
                .cells
                .as_ref()
                .unwrap(),
            &["last", "tail"]
        );
    }
    #[test]
    fn oversized_records_are_reported_without_unbounded_storage() {
        let source = Arc::new(FileSource::from_bytes(
            format!("a,b\n{},x\nend,y\n", "x".repeat(200000)).into_bytes(),
        ));
        let index = CsvIndex {
            state: Mutex::new(State {
                anchors: vec![0],
                ..Default::default()
            }),
        };
        let rows = index.page(&source, b',', 0, 3).unwrap();
        assert!(rows[1].cells.is_err());
        assert_eq!(rows[2].cells.as_ref().unwrap(), &["end", "y"]);
    }
}
