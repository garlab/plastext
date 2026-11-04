//! Resumable UAX #29 navigation. A pathological cluster or regional-indicator
//! run can require unbounded context; each poll admits only a bounded number
//! of small chunks and leaves the cursor ready to resume after I/O or a yield.
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

use crate::{ByteSource, TextError, TextSnapshot, text::crossing_range};

const CHUNK: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Backward,
    Forward,
}

pub struct GraphemeSearch {
    snapshot: TextSnapshot,
    cursor: GraphemeCursor,
    direction: Direction,
    checked: bool,
    chunk: Option<Chunk>,
    context: Option<usize>,
    complete: Option<u64>,
}

struct Chunk {
    start: usize,
    text: String,
}

impl TextSnapshot {
    /// Start one cursor step without I/O. Poll the search with a small work
    /// budget; load Missing ranges on a worker or resume Pending next frame.
    pub fn grapheme_search(
        &self,
        offset: u64,
        direction: Direction,
    ) -> Result<GraphemeSearch, TextError> {
        if offset > self.len() {
            return Err(TextError::OutOfBounds);
        }
        let len = usize::try_from(self.len()).map_err(|_| TextError::OutOfBounds)?;
        Ok(GraphemeSearch {
            snapshot: self.clone(),
            cursor: GraphemeCursor::new(offset as usize, len, true),
            direction,
            checked: false,
            chunk: None,
            context: None,
            complete: None,
        })
    }
}

impl GraphemeSearch {
    /// Some(offset) is the next/previous boundary, or the unchanged endpoint at
    /// EOF/BOF. None yields after `chunks` units of work. Missing preserves all
    /// search state; retry this same search after loading its snapshot range.
    pub fn poll(&mut self, chunks: usize) -> Result<Option<u64>, TextError> {
        if let Some(offset) = self.complete {
            return Ok(Some(offset));
        }
        if chunks == 0 {
            return Ok(None);
        }
        if !self.checked {
            self.snapshot
                .require_scalar_boundary(self.cursor.cur_cursor() as u64)?;
            self.checked = true;
        }
        if (self.direction == Direction::Backward && self.cursor.cur_cursor() == 0)
            || (self.direction == Direction::Forward
                && self.cursor.cur_cursor() as u64 == self.snapshot.len())
        {
            self.complete = Some(self.cursor.cur_cursor() as u64);
            return Ok(self.complete);
        }
        for _ in 0..chunks {
            if let Some(end) = self.context {
                let chunk = read_chunk(&self.snapshot, end as u64, true)?;
                self.cursor.provide_context(&chunk.text, chunk.start);
                self.context = None;
                continue;
            }
            if self.chunk.is_none() {
                self.chunk = Some(read_chunk(
                    &self.snapshot,
                    self.cursor.cur_cursor() as u64,
                    self.direction == Direction::Backward,
                )?);
            }
            let chunk = self.chunk.as_ref().unwrap();
            let result = match self.direction {
                Direction::Forward => self.cursor.next_boundary(&chunk.text, chunk.start),
                Direction::Backward => self.cursor.prev_boundary(&chunk.text, chunk.start),
            };
            match result {
                Ok(offset) => {
                    self.complete = Some(offset.unwrap_or(self.cursor.cur_cursor()) as u64);
                    return Ok(self.complete);
                }
                Err(GraphemeIncomplete::PreContext(end)) => self.context = Some(end),
                Err(GraphemeIncomplete::NextChunk | GraphemeIncomplete::PrevChunk) => {
                    self.chunk = None
                }
                Err(GraphemeIncomplete::InvalidOffset) => return Err(TextError::InsideScalar),
            }
        }
        Ok(None)
    }

    /// The exact immutable version used by this search, for background prefetch.
    pub fn snapshot(&self) -> &TextSnapshot {
        &self.snapshot
    }
}

fn read_chunk(snapshot: &TextSnapshot, offset: u64, before: bool) -> Result<Chunk, TextError> {
    let (start, end) = if before {
        (offset.saturating_sub(CHUNK), offset)
    } else {
        (offset, (offset + CHUNK).min(snapshot.len()))
    };
    let base = start.saturating_sub(3);
    let bytes = snapshot.bytes(base..(end + 3).min(snapshot.len()))?;
    let start = (start - base) as usize;
    let end = (end - base) as usize;
    let start = crossing_range(&bytes, start).map_or(start, |range| range.start);
    let end = crossing_range(&bytes, end).map_or(end, |range| range.start);
    let bytes = &bytes[start..end];
    let mut text = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        // One control sentinel per invalid byte preserves all byte coordinates
        // and makes each damaged byte independently selectable/deletable.
        text.extend(std::iter::repeat_n('\0', chunk.invalid().len()));
    }
    Ok(Chunk {
        start: base as usize + start,
        text,
    })
}

#[cfg(test)]
mod tests;
