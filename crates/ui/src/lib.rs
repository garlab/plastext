//! GPUI views: workspace, sidebar, tree, tabs, splits, palette and banners (spec §4.1).

mod file_tree;
mod layout;
mod quick_open;
mod search_panel;
mod tab_view;
mod workspace;

pub use workspace::{Workspace, init};
