//! Native Markdown model. Parsing and source mapping never depend on GPUI.
mod links;
mod parser;
mod search;

use std::{ops::Range, path::Path, sync::Arc};

pub use links::{LinkTarget, resolve_link};
pub use parser::{ParseSummary, parse, parse_stream};
pub use search::{RenderedMatch, find_rendered};

/// Bounds the work needed to lay out any one virtualized item.
pub const BLOCK_TEXT_LIMIT: usize = 16 * 1024;

pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "mdx"
        )
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub strike: bool,
    pub code: bool,
    pub dim: bool,
    pub math: bool,
    pub link: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// UTF-8 byte range in the block's rendered text.
    pub range: Range<usize>,
    /// Original Markdown bytes, including any inline delimiters.
    pub source: Range<usize>,
    pub style: InlineStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading {
        level: u8,
        id: String,
    },
    Code {
        language: String,
        first: bool,
    },
    Math,
    Rule,
    FrontMatter,
    TableRow {
        header: bool,
        cells: Vec<Range<usize>>,
        alignments: Arc<[Alignment]>,
    },
    Image {
        destination: String,
        title: String,
    },
    Html,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub source: Range<usize>,
    pub text: String,
    pub spans: Vec<Span>,
    pub indent: usize,
    pub marker: Option<String>,
    pub quote: Option<String>,
    pub task: Option<bool>,
}

impl Block {
    pub fn new(kind: BlockKind, source: Range<usize>) -> Self {
        Self {
            kind,
            source,
            text: String::new(),
            spans: Vec::new(),
            indent: 0,
            marker: None,
            quote: None,
            task: None,
        }
    }

    /// Maps rendered text (including entities/escapes) back to its source.
    pub fn source_at(&self, byte: usize) -> usize {
        self.spans
            .iter()
            .find(|span| span.range.contains(&byte))
            .map_or(self.source.start, |span| {
                if span.range.len() == span.source.len() {
                    span.source.start + byte.saturating_sub(span.range.start)
                } else {
                    span.source.start
                }
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub text: String,
    pub id: String,
    pub level: u8,
    pub source: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub blocks: Vec<Arc<Block>>,
    pub headings: Vec<Heading>,
    pub warnings: Vec<String>,
}

impl Document {
    pub fn block_at(&self, byte: usize) -> usize {
        self.blocks
            .partition_point(|block| block.source.start <= byte)
            .saturating_sub(1)
    }
    pub fn anchor(&self, id: &str) -> Option<usize> {
        self.headings
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.source.start)
    }
}
