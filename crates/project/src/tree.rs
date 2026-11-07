use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    /// Matched by `.gitignore`, `.ignore` or another ignore file.
    pub ignored: bool,
}

/// How entries matched by ignore files are listed (spec PRJ-3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShowIgnored {
    Hide,
    #[default]
    Dim,
    Show,
}

/// Which entries directory listings include (spec PRJ-3).
#[derive(Clone, Debug)]
pub struct TreeOptions {
    pub show_hidden: bool,
    pub show_ignored: ShowIgnored,
    /// Always hidden; matched against paths relative to the project root.
    pub exclude: GlobSet,
}

impl TreeOptions {
    pub fn new(
        show_hidden: bool,
        show_ignored: ShowIgnored,
        exclude: &[impl AsRef<str>],
    ) -> Result<Self, globset::Error> {
        let mut globs = GlobSetBuilder::new();
        for pattern in exclude {
            globs.add(Glob::new(pattern.as_ref())?);
        }
        Ok(Self {
            show_hidden,
            show_ignored,
            exclude: globs.build()?,
        })
    }
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self::new(false, ShowIgnored::Dim, &["**/.git", "**/.DS_Store"])
            .expect("the default exclude globs are valid")
    }
}

/// Lists `dir`, a directory inside the project at `root`: directories first,
/// then case-insensitively by name. Blocks: call it off the main thread.
pub fn read_dir(root: &Path, dir: &Path, options: &TreeOptions) -> io::Result<Vec<DirEntry>> {
    let unignored = (options.show_ignored != ShowIgnored::Show).then(|| {
        WalkBuilder::new(dir)
            .max_depth(Some(1))
            .hidden(false)
            .build()
            .filter_map(Result::ok)
            .map(ignore::DirEntry::into_path)
            .collect::<HashSet<_>>()
    });

    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        // An entry that vanished or can't be inspected is simply left out.
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !options.show_hidden && name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if options
            .exclude
            .is_match(path.strip_prefix(root).unwrap_or(&path))
        {
            continue;
        }
        let ignored = unignored
            .as_ref()
            .is_some_and(|unignored| !unignored.contains(&path));
        if ignored && options.show_ignored == ShowIgnored::Hide {
            continue;
        }
        let is_dir = match entry.file_type() {
            Ok(file_type) if file_type.is_symlink() => path.is_dir(),
            Ok(file_type) => file_type.is_dir(),
            Err(_) => false,
        };
        let kind = if is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        entries.push(DirEntry {
            path,
            name,
            kind,
            ignored,
        });
    }
    entries.sort_by_cached_key(|entry| {
        (
            entry.kind != EntryKind::Dir,
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
    Ok(entries)
}

/// The project tree as shown in the sidebar: directories load lazily when
/// expanded, and the visible entries are flattened into rows (spec PRJ-2).
pub struct FileTree {
    root: PathBuf,
    children: HashMap<PathBuf, Vec<Arc<DirEntry>>>,
    expanded: HashSet<PathBuf>,
    rows: Vec<TreeRow>,
    visible_directories: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct TreeRow {
    pub entry: Arc<DirEntry>,
    pub depth: usize,
    pub expanded: bool,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        Self {
            visible_directories: vec![root.clone()],
            root,
            children: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }
    /// Only directories whose children are currently displayed, including root.
    pub fn visible_directories(&self) -> &[PathBuf] {
        &self.visible_directories
    }

    pub fn is_loaded(&self, dir: &Path) -> bool {
        self.children.contains_key(dir)
    }
    pub fn expand(&mut self, dir: &Path) {
        self.expanded.insert(dir.to_owned());
        self.rebuild();
    }
    pub fn forget(&mut self, path: &Path) {
        self.children.retain(|dir, _| !dir.starts_with(path));
        self.expanded.retain(|dir| !dir.starts_with(path));
        self.rebuild();
    }

    /// Records the listing of `dir`, as returned by [`read_dir`].
    /// Returns removed/retyped paths so asynchronous consumers can retire
    /// listings and selection state below them.
    pub fn set_children(&mut self, dir: &Path, entries: Vec<DirEntry>) -> Vec<PathBuf> {
        let current: HashMap<_, _> = entries
            .iter()
            .map(|entry| (&entry.path, entry.kind))
            .collect();
        let removed: Vec<_> = self
            .children
            .get(dir)
            .into_iter()
            .flatten()
            .filter(|entry| {
                current
                    .get(&entry.path)
                    .is_none_or(|kind| *kind != entry.kind)
            })
            .map(|entry| entry.path.clone())
            .collect();
        if !removed.is_empty() {
            self.children
                .retain(|path, _| !removed.iter().any(|removed| path.starts_with(removed)));
            self.expanded
                .retain(|path| !removed.iter().any(|removed| path.starts_with(removed)));
        }
        let entries = entries.into_iter().map(Arc::new).collect();
        self.children.insert(dir.to_owned(), entries);
        self.rebuild();
        removed
    }

    /// Expands or collapses `dir`. Returns whether it must be listed first, in
    /// which case its rows appear once [`Self::set_children`] is called.
    pub fn toggle(&mut self, dir: &Path) -> bool {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_owned());
        }
        self.rebuild();
        self.expanded.contains(dir) && !self.is_loaded(dir)
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len());
        let mut directories = vec![self.root.clone()];
        self.push_rows(&self.root, 0, &mut rows, &mut directories);
        self.rows = rows;
        self.visible_directories = directories;
    }

    fn push_rows(
        &self,
        dir: &Path,
        depth: usize,
        rows: &mut Vec<TreeRow>,
        directories: &mut Vec<PathBuf>,
    ) {
        let Some(children) = self.children.get(dir) else {
            return;
        };
        for child in children {
            let expanded = child.kind == EntryKind::Dir && self.expanded.contains(&child.path);
            rows.push(TreeRow {
                entry: child.clone(),
                depth,
                expanded,
            });
            if expanded {
                directories.push(child.path.clone());
                self.push_rows(&child.path, depth + 1, rows, directories);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [".git", "z", "target", "z/inner"] {
            fs::create_dir(root.join(path)).unwrap();
        }
        let files = [
            (".gitignore", "target/\n*.log\n"),
            ("b.txt", ""),
            ("A.txt", ""),
            ("x.log", ""),
            (".hidden", ""),
            (".DS_Store", ""),
            ("z/c.txt", ""),
        ];
        for (path, contents) in files {
            fs::write(root.join(path), contents).unwrap();
        }
        dir
    }

    fn names(entries: &[DirEntry]) -> Vec<(&str, bool)> {
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.ignored))
            .collect()
    }

    #[test]
    fn lists_directories_first_and_marks_ignored_entries() {
        let project = project();
        let root = project.path();
        let entries = read_dir(root, root, &TreeOptions::default()).unwrap();
        assert_eq!(
            names(&entries),
            [
                ("target", true),
                ("z", false),
                ("A.txt", false),
                ("b.txt", false),
                ("x.log", true)
            ]
        );
        assert_eq!(entries[1].kind, EntryKind::Dir);
        assert_eq!(entries[2].kind, EntryKind::File);
    }

    #[test]
    fn honors_hidden_and_ignored_options() {
        let project = project();
        let root = project.path();
        let options = TreeOptions::new(true, ShowIgnored::Hide, &["**/.git"]).unwrap();
        let entries = read_dir(root, root, &options).unwrap();
        assert_eq!(
            names(&entries),
            [
                ("z", false),
                (".DS_Store", false),
                (".gitignore", false),
                (".hidden", false),
                ("A.txt", false),
                ("b.txt", false)
            ]
        );
    }

    #[test]
    fn expanding_and_collapsing_directories_updates_rows() {
        let project = project();
        let root = project.path();
        let options = TreeOptions::default();
        let mut tree = FileTree::new(root.to_owned());
        tree.set_children(root, read_dir(root, root, &options).unwrap());
        assert_eq!(tree.rows().len(), 5);

        let z = root.join("z");
        assert!(tree.toggle(&z), "z isn't listed yet");
        tree.set_children(&z, read_dir(root, &z, &options).unwrap());
        let rows: Vec<(&str, usize)> = tree
            .rows()
            .iter()
            .map(|row| (row.entry.name.as_str(), row.depth))
            .collect();
        assert_eq!(
            rows,
            [
                ("target", 0),
                ("z", 0),
                ("inner", 1),
                ("c.txt", 1),
                ("A.txt", 0),
                ("b.txt", 0),
                ("x.log", 0)
            ]
        );
        assert!(tree.rows()[1].expanded);

        assert!(!tree.toggle(&z), "collapsing never needs a listing");
        assert_eq!(tree.rows().len(), 5);
        assert!(!tree.toggle(&z), "z's listing is kept while collapsed");
        assert_eq!(tree.rows().len(), 7);
    }

    #[test]
    fn refreshing_removed_or_retyped_folders_discards_cached_descendants() {
        let root = PathBuf::from("project");
        let dir = root.join("dir");
        let other = root.join("other");
        let entry = |path: &Path, kind| DirEntry {
            path: path.to_owned(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            kind,
            ignored: false,
        };
        for replacement in [None, Some(EntryKind::File)] {
            let mut tree = FileTree::new(root.clone());
            tree.set_children(
                &root,
                vec![entry(&dir, EntryKind::Dir), entry(&other, EntryKind::Dir)],
            );
            for path in [&dir, &other] {
                tree.expand(path);
                tree.set_children(path, vec![entry(&path.join("child"), EntryKind::File)]);
            }
            assert_eq!(
                tree.visible_directories(),
                [root.clone(), dir.clone(), other.clone()]
            );
            let mut entries = vec![entry(&other, EntryKind::Dir)];
            if let Some(kind) = replacement {
                entries.push(entry(&dir, kind));
            }
            assert_eq!(
                tree.set_children(&root, entries),
                std::slice::from_ref(&dir)
            );
            assert!(!tree.is_loaded(&dir));
            assert!(tree.is_loaded(&other));
            assert_eq!(tree.visible_directories(), [root.clone(), other.clone()]);
            tree.set_children(
                &root,
                vec![entry(&dir, EntryKind::Dir), entry(&other, EntryKind::Dir)],
            );
            assert!(tree.toggle(&dir), "recreated folder must be listed again");
            assert!(
                !tree
                    .rows()
                    .iter()
                    .any(|r| r.entry.path == dir.join("child"))
            );
            assert!(
                tree.rows()
                    .iter()
                    .any(|r| r.entry.path == other.join("child"))
            );
        }
    }
}
