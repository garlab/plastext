//! Text view element: layout, wrapping, selection, input, gutter and folding (spec §5.3).

mod text_view;
mod theme;

pub use text_view::{TextStyle, TextView, TextViewStatus, init};
pub use theme::Theme;
