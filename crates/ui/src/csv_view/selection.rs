use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Position {
    pub row: u64,
    pub column: usize,
    /// Byte offset in the decoded cell, before replacing newlines for display.
    pub byte: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Selection {
    #[default]
    None,
    Text {
        anchor: Position,
        head: Position,
    },
    Rows {
        anchor: u64,
        ranges: Vec<Range<u64>>,
    },
    All,
}

#[derive(Clone, Copy)]
pub(super) enum Unit {
    Character,
    Word,
    Cell,
}

#[derive(Clone)]
pub(super) enum Drag {
    Text { anchor: Range<Position>, unit: Unit },
    Rows,
}

impl Selection {
    pub fn text_range(&self) -> Option<Range<Position>> {
        match *self {
            Self::Text { anchor, head } if anchor != head => {
                Some(anchor.min(head)..anchor.max(head))
            }
            _ => None,
        }
    }

    pub fn rows(&self, count: u64) -> Vec<Range<u64>> {
        match self {
            Self::Rows { ranges, .. } => ranges.clone(),
            Self::All if count > 0 => std::iter::once(0..count).collect(),
            _ => self.text_range().map_or_else(Vec::new, |r| {
                std::iter::once(r.start.row..r.end.row + 1).collect()
            }),
        }
    }

    pub fn row_selected(&self, row: u64) -> bool {
        match self {
            Self::All => true,
            Self::Rows { ranges, .. } => {
                let i = ranges.partition_point(|range| range.end <= row);
                ranges.get(i).is_some_and(|range| range.contains(&row))
            }
            _ => false,
        }
    }

    pub fn cell_range(&self, row: u64, column: usize, len: usize) -> Option<Range<usize>> {
        let range = self.text_range()?;
        let cell = (row, column);
        if cell < (range.start.row, range.start.column) || cell > (range.end.row, range.end.column)
        {
            return None;
        }
        let start = if cell == (range.start.row, range.start.column) {
            range.start.byte.min(len)
        } else {
            0
        };
        let end = if cell == (range.end.row, range.end.column) {
            range.end.byte.min(len)
        } else {
            len
        };
        Some(start..end)
    }

    pub fn select_row(&mut self, row: u64, extend: bool, toggle: bool) {
        if extend {
            let anchor = match self {
                Self::Rows { anchor, .. } => *anchor,
                Self::Text { anchor, .. } => anchor.row,
                _ => row,
            };
            *self = Self::Rows {
                anchor,
                ranges: std::iter::once(anchor.min(row)..anchor.max(row) + 1).collect(),
            };
        } else if toggle {
            let mut ranges = match self {
                Self::Rows { ranges, .. } => std::mem::take(ranges),
                _ => Vec::new(),
            };
            let i = ranges.partition_point(|range| range.end <= row);
            if ranges.get(i).is_some_and(|range| range.contains(&row)) {
                let range = ranges.remove(i);
                if row + 1 < range.end {
                    ranges.insert(i, row + 1..range.end);
                }
                if range.start < row {
                    ranges.insert(i, range.start..row);
                }
            } else {
                ranges.insert(i, row..row + 1);
                let mut merged: Vec<Range<u64>> = Vec::new();
                for range in ranges {
                    if let Some(last) = merged.last_mut()
                        && last.end == range.start
                    {
                        last.end = range.end;
                    } else {
                        merged.push(range);
                    }
                }
                ranges = merged;
            }
            *self = Self::Rows {
                anchor: row,
                ranges,
            };
        } else {
            *self = Self::Rows {
                anchor: row,
                ranges: std::iter::once(row..row + 1).collect(),
            };
        }
    }

    pub fn extend_text(&mut self, anchor: Range<Position>, target: Range<Position>) {
        *self = if target.start < anchor.start {
            Self::Text {
                anchor: anchor.end,
                head: target.start,
            }
        } else {
            Self::Text {
                anchor: anchor.start,
                head: target.end,
            }
        };
    }
}

pub(super) fn unit_range(text: &str, mut at: Position, unit: Unit) -> Range<Position> {
    at.byte = text.floor_char_boundary(at.byte.min(text.len()));
    let bytes = match unit {
        Unit::Character => at.byte..at.byte,
        Unit::Cell => 0..text.len(),
        Unit::Word => word_at(text, at.byte),
    };
    Position {
        byte: bytes.start,
        ..at
    }..Position {
        byte: bytes.end,
        ..at
    }
}

fn word_at(text: &str, byte: usize) -> Range<usize> {
    let Some((start, ch)) = text
        .char_indices()
        .find(|(i, c)| i + c.len_utf8() > byte)
        .or_else(|| text.char_indices().last())
    else {
        return 0..0;
    };
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let kind = class(ch);
    let left = text[..start]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(start, |(i, _)| i);
    let right = text[start..]
        .char_indices()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(start, |(i, c)| start + i + c.len_utf8());
    left..right
}

pub(super) fn display_offset(source: &str, byte: usize) -> usize {
    let byte = source.floor_char_boundary(byte.min(source.len()));
    byte + source[..byte]
        .bytes()
        .filter(|b| matches!(b, b'\r' | b'\n'))
        .count()
        * 4
}

pub(super) fn source_offset(source: &str, displayed: usize) -> usize {
    let mut extra = 0;
    for (byte, _) in source.match_indices(['\r', '\n']) {
        let start = byte + extra;
        if displayed < start {
            break;
        }
        if displayed < start + 5 {
            return byte + usize::from(displayed - start >= 3);
        }
        extra += 4;
    }
    source.floor_char_boundary(displayed.saturating_sub(extra).min(source.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pos(row: u64, column: usize, byte: usize) -> Position {
        Position { row, column, byte }
    }

    #[test]
    fn row_ranges_extend_toggle_split_and_merge_without_materializing_rows() {
        let mut s = Selection::None;
        s.select_row(20_000_000, false, false);
        s.select_row(1, true, false);
        assert_eq!(s.rows(0), vec![1..20_000_001]);
        s.select_row(3, false, true);
        assert_eq!(s.rows(0), vec![1..3, 4..20_000_001]);
        assert!(!s.row_selected(3));
        assert!(s.row_selected(4));
        s.select_row(3, false, true);
        assert_eq!(s.rows(0), vec![1..20_000_001]);
        s.select_row(0, false, true);
        assert_eq!(s.rows(0), vec![0..20_000_001]);
        s.select_row(9, false, false);
        s.select_row(9, false, true);
        assert!(s.rows(0).is_empty());
    }

    #[test]
    fn backwards_text_and_word_drags_keep_the_original_anchor() {
        let mut s = Selection::None;
        s.extend_text(pos(2, 1, 2)..pos(2, 1, 5), pos(1, 0, 1)..pos(1, 0, 4));
        assert_eq!(s.cell_range(1, 0, 10), Some(1..10));
        assert_eq!(s.cell_range(2, 0, 8), Some(0..8));
        assert_eq!(s.cell_range(2, 1, 10), Some(0..5));
        assert_eq!(s.cell_range(2, 2, 5), None);
        assert_eq!(s.rows(0), vec![1..3]);
    }

    #[test]
    fn word_and_cell_selection_preserve_unicode_boundaries() {
        let text = "a café_界! 😀";
        assert_eq!(word_at(text, 6), 2..11);
        assert_eq!(word_at(text, 11), 11..12);
        assert_eq!(word_at(text, text.len()), 13..17);
        assert_eq!(
            unit_range(text, pos(0, 0, 3), Unit::Cell),
            pos(0, 0, 0)..pos(0, 0, text.len())
        );
    }

    #[test]
    fn displayed_newline_markers_map_back_to_original_bytes() {
        let text = "é\r\n界";
        for byte in [0, 2, 3, 4, 7] {
            assert_eq!(source_offset(text, display_offset(text, byte)), byte);
        }
        assert_eq!(source_offset(text, 3), 2);
        assert_eq!(source_offset(text, 5), 3);
        assert_eq!(source_offset(text, usize::MAX), text.len());
    }
}
