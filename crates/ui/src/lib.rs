//! GPUI views: workspace, sidebar, tree, tabs, splits, palette and banners (spec §4.1).

mod csv_view;
mod file_icons;
mod file_tree;
mod icon_themes;
mod image_view;
mod layout;
mod markdown_view;
mod pdf_view;
mod picker;
mod quick_open;
mod search_panel;
mod sgf_view;
mod tab_view;
mod tex_view;
mod workspace;
mod zoom;

pub use file_icons::Assets;
pub use workspace::{Workspace, init};
