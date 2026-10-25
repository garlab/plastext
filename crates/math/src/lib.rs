//! TeX math parsing and layout (spec MD-3).

#[cfg(feature = "typst-engine")]
mod typst_engine;

#[cfg(feature = "typst-engine")]
pub use typst_engine::{Fonts, Layout, TypstMath};
