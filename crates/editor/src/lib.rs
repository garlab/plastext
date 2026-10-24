//! Text view element: layout, wrapping, selection, input, gutter and folding (spec §5.3).

mod hex_view;
mod row_text;
mod text_input;
mod text_view;
mod theme;

use gpui::App;

pub use hex_view::{HexView, HexViewStatus};
pub use text_input::{TextInput, TextInputEvent};
pub use text_view::{TextStyle, TextView, TextViewStatus};
pub use theme::Theme;

/// Registers the editor's key bindings.
pub fn init(cx: &mut App) {
    hex_view::init(cx);
    text_input::init(cx);
    text_view::init(cx);
}
