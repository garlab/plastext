//! Unicode coordinates over byte-preserving snapshots. UTF-16 positions inside
//! a surrogate pair round to the explicit bias; byte positions inside a valid
//! UTF-8 scalar are rejected. Each malformed byte is one addressable unit.
use std::ops::Range;

use crate::{ByteSource, EditBuffer, EditError, ReadError, TextSnapshot, UndoGroup, piece_tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    OutOfBounds,
    InsideScalar,
    Read(ReadError),
    Edit(EditError),
}

impl From<ReadError> for TextError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl TextSnapshot {
    pub fn scalar_count(&self) -> u64 {
        self.root.as_ref().map_or(0, |node| node.unicode.scalars)
    }
    pub fn utf16_len(&self) -> u64 {
        self.root.as_ref().map_or(0, |node| node.unicode.utf16)
    }

    pub fn is_scalar_boundary(&self, offset: u64) -> Result<bool, TextError> {
        if offset > self.len() {
            return Err(TextError::OutOfBounds);
        }
        if offset == 0 || offset == self.len() {
            return Ok(true);
        }
        let start = offset.saturating_sub(3);
        let bytes = self.bytes(start..(offset + 3).min(self.len()))?;
        Ok(crossing_range(&bytes, (offset - start) as usize).is_none())
    }

    pub fn byte_to_scalar(&self, offset: u64) -> Result<u64, TextError> {
        self.require_scalar_boundary(offset)?;
        Ok(piece_tree::unicode_prefix(&self.root, offset)?.scalars)
    }

    /// Resolve a rebased byte anchor that may now fall inside a newly joined
    /// scalar (for example after deleting between damaged UTF-8 fragments).
    pub fn clip_to_scalar(&self, offset: u64, bias: Bias) -> Result<u64, TextError> {
        if offset > self.len() {
            return Err(TextError::OutOfBounds);
        }
        if offset == 0 || offset == self.len() {
            return Ok(offset);
        }
        let start = offset.saturating_sub(3);
        let bytes = self.bytes(start..(offset + 3).min(self.len()))?;
        Ok(
            crossing_range(&bytes, (offset - start) as usize).map_or(offset, |range| {
                start
                    + if bias == Bias::Before {
                        range.start
                    } else {
                        range.end
                    } as u64
            }),
        )
    }

    pub fn byte_to_utf16(&self, offset: u64) -> Result<u64, TextError> {
        self.require_scalar_boundary(offset)?;
        Ok(piece_tree::unicode_prefix(&self.root, offset)?.utf16)
    }

    pub fn scalar_to_byte(&self, scalar: u64) -> Result<u64, TextError> {
        piece_tree::unicode_offset(&self.root, scalar, false, false)?.ok_or(TextError::OutOfBounds)
    }

    pub fn utf16_to_byte(&self, units: u64, bias: Bias) -> Result<u64, TextError> {
        piece_tree::unicode_offset(&self.root, units, true, bias == Bias::After)?
            .ok_or(TextError::OutOfBounds)
    }

    pub(super) fn require_scalar_boundary(&self, offset: u64) -> Result<(), TextError> {
        if self.is_scalar_boundary(offset)? {
            Ok(())
        } else {
            Err(TextError::InsideScalar)
        }
    }
}

/// At most one valid scalar can cross a byte boundary. The caller supplies up
/// to three bytes before and after it, including across piece boundaries.
pub(super) fn crossing_range(bytes: &[u8], at: usize) -> Option<Range<usize>> {
    for start in at.saturating_sub(3)..at {
        let (_, len) = crate::unicode_summary::scalar(&bytes[start..]);
        if start + len > at {
            return Some(start..start + len);
        }
    }
    None
}

impl EditBuffer {
    /// Character-safe text replacement. IME may replace part of a grapheme,
    /// but it must never bisect a UTF-8 scalar. Use grapheme navigation to
    /// choose ranges for normal cursor movement and backspace/delete.
    pub fn replace_text(
        &mut self,
        range: Range<u64>,
        text: &str,
        group: UndoGroup,
    ) -> Result<(), TextError> {
        if range.start > range.end {
            return Err(TextError::OutOfBounds);
        }
        let snapshot = self.snapshot();
        snapshot.require_scalar_boundary(range.start)?;
        snapshot.require_scalar_boundary(range.end)?;
        self.replace(range, text.as_bytes(), group)
            .map_err(|error| match error {
                EditError::Read(read) => TextError::Read(read),
                other => TextError::Edit(other),
            })
    }
}

#[cfg(test)]
mod tests;
