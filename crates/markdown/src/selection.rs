//! Rendered coordinates stay valid when parsed pages leave the cache.
use crate::{Block, BlockKind, BlockType, DocumentIndex};
use plastext_buffer::FileSource;
use std::{collections::HashSet, io, ops::Range};
use unicode_segmentation::UnicodeSegmentation as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub block: usize,
    pub byte: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    Text {
        anchor: Position,
        head: Position,
    },
    All,
}
impl Selection {
    pub fn range(&self) -> Option<Range<Position>> {
        match *self {
            Self::None => None,
            Self::All => Some(
                Position::default()..Position {
                    block: usize::MAX,
                    byte: usize::MAX,
                },
            ),
            Self::Text { anchor, head } => Some(anchor.min(head)..anchor.max(head)),
        }
    }
    pub fn block_range(&self, block: usize, len: usize) -> Option<Range<usize>> {
        let range = self.range()?;
        if block < range.start.block || block > range.end.block {
            return None;
        }
        let start = if block == range.start.block {
            range.start.byte.min(len)
        } else {
            0
        };
        let end = if block == range.end.block {
            range.end.byte.min(len)
        } else {
            len
        };
        (start < end).then_some(start..end)
    }
    pub fn extend(&mut self, anchor: Range<Position>, target: Range<Position>) {
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

#[derive(Clone, Copy, Debug)]
pub enum Unit {
    Character,
    Word,
    Block,
}

pub fn unit_range(text: &str, at: usize, unit: Unit) -> Range<usize> {
    let at = text.floor_char_boundary(at.min(text.len()));
    match unit {
        Unit::Character => {
            let at = text
                .grapheme_indices(true)
                .find(|(start, part)| *start <= at && at < start + part.len())
                .map_or(text.len(), |(start, _)| start);
            at..at
        }
        Unit::Block => 0..text.len(),
        Unit::Word => text
            .split_word_bound_indices()
            .map(|(start, part)| start..start + part.len())
            .find(|range| range.contains(&at) || (at == text.len() && range.end == at))
            .unwrap_or(at..at),
    }
}

/// Copy one temporary parsed page at a time, independently of the view cache.
/// No truncated result reaches the clipboard on cancellation, file changes or limits.
pub fn copy_selection(
    document: &DocumentIndex,
    source: &FileSource,
    selection: &Selection,
    expanded: &HashSet<usize>,
    cancelled: impl Fn() -> bool,
    limit: usize,
) -> io::Result<String> {
    let mut output = String::new();
    let Some(range) = selection.range().filter(|r| !r.is_empty()) else {
        return Ok(output);
    };
    let mut previous: Option<BlockType> = None;
    let first = document
        .blocks
        .get(range.start.block)
        .map_or(0, |b| b.page as usize);
    let last = document
        .blocks
        .get(range.end.block)
        .map_or(document.pages.len(), |b| b.page as usize + 1);
    for page in first..last {
        if cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let blocks = document.parse_page(source, page, &cancelled)?;
        for (local, block) in blocks.iter().enumerate() {
            if cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let ix = document.pages[page].blocks.start + local;
            if !crate::BlockEntry::visible(&document.blocks, ix, expanded) {
                continue;
            }
            let Some(selected) = selection.block_range(ix, block.text.len()) else {
                continue;
            };
            if let Some(previous) = &previous {
                let separator = match (&block.kind, previous) {
                    (BlockKind::Code { first: false, .. }, BlockType::Code) => "",
                    (BlockKind::TableRow { .. }, BlockType::TableRow | BlockType::TableHead) => {
                        "\n"
                    }
                    (BlockKind::FrontMatterRow { .. }, BlockType::MetadataRow) => "\n",
                    _ if block.marker.is_some() => "\n",
                    _ => "\n\n",
                };
                append(&mut output, separator, limit)?;
            }
            // The parser already separates table cells with tabs, including
            // empty cells. Only its final synthetic cell terminator is omitted.
            let end = if let BlockKind::TableRow { cells, .. } = &block.kind {
                selected.end.min(cells.last().map_or(0, |cell| cell.end))
            } else {
                selected.end
            };
            append(
                &mut output,
                slice(block, selected.start.min(end)..end),
                limit,
            )?;
            previous = Some(document.blocks[ix].kind);
        }
    }
    Ok(output)
}
fn slice(block: &Block, range: Range<usize>) -> &str {
    &block.text
        [block.text.floor_char_boundary(range.start)..block.text.floor_char_boundary(range.end)]
}
fn append(output: &mut String, value: &str, limit: usize) -> io::Result<()> {
    if output.len().saturating_add(value.len()) > limit {
        return Err(io::Error::other(
            "selection exceeds the 64 MB copy limit; select a smaller range",
        ));
    }
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn copy(text: &str, selection: Selection, limit: usize) -> io::Result<String> {
        let source = FileSource::from_bytes(text.as_bytes().to_vec());
        let doc = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
        copy_selection(&doc, &source, &selection, &HashSet::new(), || false, limit)
    }
    #[test]
    fn rendered_copy_keeps_unicode_tex_code_and_table_columns() {
        let text = "# A *heading* $x^2$\n\nCafé &amp; [link](https://example.com).\n\n$$\\frac{a}{b}$$\n\n| A | B |\n| - | - |\n| one | two |\n\n```rs\nlet x = 1;\n```";
        let copied = copy(text, Selection::All, 4096).unwrap();
        assert!(copied.starts_with("A heading x^2\n\nCafé & link."));
        assert!(copied.contains(r"\frac{a}{b}"));
        assert!(copied.contains("A\tB\none\ttwo"));
        assert!(copied.ends_with("let x = 1;\n"));
    }
    #[test]
    fn reversed_selection_and_collapsed_metadata() {
        let selection = Selection::Text {
            anchor: Position { block: 1, byte: 3 },
            head: Position { block: 0, byte: 3 },
        };
        assert_eq!(copy("café!\n\nend.", selection, 100).unwrap(), "é!\n\nend");
        assert_eq!(
            copy("---\nsecret: value\n---\n\nBody", Selection::All, 100).unwrap(),
            "Body"
        );
        assert_eq!(unit_range("é界 x", 2, Unit::Word), 2..5);
        assert_eq!(unit_range("e\u{301}lan", 1, Unit::Word), 0..6);
        assert_eq!(unit_range("e\u{301}", 1, Unit::Character), 0..0);
        assert_eq!(unit_range("👩‍💻!", 4, Unit::Word), 0..11);
    }
    #[test]
    fn empty_table_cells_and_reversed_word_drags_preserve_boundaries() {
        let text = "| A | B | C |\n| - | - | - |\n| | x | |\n| | | |";
        assert_eq!(
            copy(text, Selection::All, 100).unwrap(),
            "A\tB\tC\n\tx\t\n\t\t"
        );
        let p = |block, byte| Position { block, byte };
        let mut selected = Selection::None;
        selected.extend(p(2, 4)..p(2, 9), p(1, 2)..p(1, 5));
        assert_eq!(selected.range(), Some(p(1, 2)..p(2, 9)));
        selected.extend(p(2, 4)..p(2, 9), p(3, 0)..p(3, 5));
        assert_eq!(selected.range(), Some(p(2, 4)..p(3, 5)));
    }
    #[test]
    fn copy_crosses_pages_and_rejects_limits_cancellation_and_changed_sources() {
        let text = "paragraph café $x^2$\n\n".repeat(10000);
        let copied = copy(&text, Selection::All, 1_000_000).unwrap();
        assert_eq!(copied, "paragraph café x^2\n\n".repeat(10000).trim_end());
        assert!(copy(&text, Selection::All, 100).is_err());
        let source = FileSource::from_bytes(text.into_bytes());
        let doc = DocumentIndex::build(&source, || false, |_, _| 0).unwrap();
        assert_eq!(
            copy_selection(
                &doc,
                &source,
                &Selection::All,
                &HashSet::new(),
                || true,
                100
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Interrupted
        );
        assert!(
            copy_selection(
                &doc,
                &FileSource::from_bytes(b"changed".to_vec()),
                &Selection::All,
                &HashSet::new(),
                || false,
                100
            )
            .is_err()
        );
    }
}
