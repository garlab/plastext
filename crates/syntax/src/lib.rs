//! tree-sitter integration, language registry, highlighter and lexical fallback (spec §5.3).

mod highlight;
mod languages;

pub use highlight::{Highlight, SyntaxTree};
pub use languages::{Language, detect, languages};
