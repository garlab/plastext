//! Bounded word movement over grapheme boundaries, including uncached files.
use crate::{ByteSource, Direction, GraphemeSearch, TextError, TextSnapshot};

pub struct WordSearch {
    text: TextSnapshot,
    direction: Direction,
    at: u64,
    group: Option<u8>,
    step: Option<GraphemeSearch>,
    done: bool,
}

impl TextSnapshot {
    pub fn word_search(&self, offset: u64, direction: Direction) -> Result<WordSearch, TextError> {
        self.require_scalar_boundary(offset)?;
        Ok(WordSearch {
            text: self.clone(),
            direction,
            at: offset,
            group: None,
            step: None,
            done: false,
        })
    }
}

impl WordSearch {
    pub fn poll(&mut self, steps: usize) -> Result<Option<u64>, TextError> {
        if self.done {
            return Ok(Some(self.at));
        }
        for _ in 0..steps {
            if self.step.is_none() {
                self.step = Some(self.text.grapheme_search(self.at, self.direction)?);
            }
            let Some(next) = self.step.as_mut().unwrap().poll(1)? else {
                return Ok(None);
            };
            if next == self.at {
                self.done = true;
                return Ok(Some(self.at));
            }
            let start = self.at.min(next);
            let end = self.at.max(next).min(start + 4);
            let bytes = self.text.bytes(start..end)?;
            let ch = bytes
                .utf8_chunks()
                .next()
                .and_then(|s| s.valid().chars().next())
                .unwrap_or('\0');
            let class = if ch.is_whitespace() {
                0
            } else if ch.is_alphanumeric() || ch == '_' {
                1
            } else {
                2
            };
            // Skip leading whitespace, then traverse one word/punctuation run.
            if self.group.is_some_and(|group| group != 0 && group != class) {
                self.done = true;
                return Ok(Some(self.at));
            }
            self.group = Some(class);
            self.at = next;
            self.step = None;
        }
        Ok(None)
    }
}
