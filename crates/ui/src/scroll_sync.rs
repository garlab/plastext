//! Source coordinates shared by the text editor and native document readers.
use plastext_editor::ScrollAnchor;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Position {
    pub source: ScrollAnchor,
    /// Expanded document coordinates preserve positions inside local includes
    /// when both panes are rendered. Source panes use the root include command.
    pub rendered: Option<RenderedAnchor>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RenderedAnchor {
    /// Distinguishes blocks emitted by the same source command (for example
    /// the title, author and date produced by LaTeX's `\maketitle`).
    pub block: usize,
    /// Expanded source coordinates also allow a fallback after reindexing.
    pub span: ScrollAnchor,
}

impl From<ScrollAnchor> for Position {
    fn from(source: ScrollAnchor) -> Self {
        Self {
            source,
            rendered: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Revision {
    Source,
    /// Identity of the parsed index. A new index needs its anchor reapplied.
    Rendered(usize),
}
