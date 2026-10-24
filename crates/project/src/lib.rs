//! Workspace model: file tree, ignore rules, watcher, file index and snapshots (spec §5.1).

mod files;
mod tree;

pub use files::walk_files;
pub use tree::{DirEntry, EntryKind, FileTree, ShowIgnored, TreeOptions, TreeRow, read_dir};
