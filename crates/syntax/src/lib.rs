//! tree-sitter integration, language registry, highlighter and lexical fallback (spec §5.3).

mod folds;
mod highlight;
mod injections;
mod languages;
mod lexical;
mod symbols;

pub use folds::Fold;
pub use highlight::{Highlight, SyntaxTree};
pub use languages::{Language, detect, language_for_label, languages};
pub use lexical::Lexical;
pub use symbols::Symbol;
