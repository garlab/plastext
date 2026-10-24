//! Text view element: layout, wrapping, selection, input, gutter and folding (spec §5.3).

mod row_text;
mod text_input;
mod text_view;
mod theme;

use gpui::App;

pub use text_input::{TextInput, TextInputEvent};
pub use text_view::{TextStyle, TextView, TextViewStatus};
pub use theme::Theme;

/// Registers the editor's key bindings.
pub fn init(cx: &mut App) {
    text_input::init(cx);
    text_view::init(cx);
}
