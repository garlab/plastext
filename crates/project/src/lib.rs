//! Workspace model: file tree, ignore rules, watcher, file index and snapshots (spec §5.1).

mod tree;

pub use tree::{DirEntry, EntryKind, FileTree, ShowIgnored, TreeOptions, TreeRow, read_dir};
