//! Read-only, UI-independent Git status. Call on a background worker.
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

mod discovery;
pub use discovery::DiscoveryCache;
pub mod watch;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileStatus {
    pub staged: bool,
    pub modified: bool,
    pub conflict: bool,
    /// Git reports a deletion in the index or working tree.
    pub deleted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoration {
    Staged,
    Modified,
    Mixed,
    Conflict,
}

impl FileStatus {
    fn merge(&mut self, other: Self) {
        self.staged |= other.staged;
        self.modified |= other.modified;
        self.conflict |= other.conflict;
        self.deleted |= other.deleted;
    }
    pub fn decoration(self) -> Option<Decoration> {
        match (self.staged, self.modified, self.conflict) {
            (_, _, true) => Some(Decoration::Conflict),
            (true, true, _) => Some(Decoration::Mixed),
            (true, false, _) => Some(Decoration::Staged),
            (false, true, _) => Some(Decoration::Modified),
            _ => None,
        }
    }
}

/// A complete status result. Only changed paths are retained; lookups perform
/// no allocation, filesystem access or Git work.
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub root: PathBuf,
    pub watch_directories: Vec<PathBuf>,
    pub watch: Option<Arc<watch::Plan>>,
    files: HashMap<PathBuf, FileStatus>,
}

/// Complete per-repository snapshots. A discovered repository without a result
/// masks its parent repository, rather than borrowing unrelated parent status.
#[derive(Clone, Debug, Default)]
pub struct RepositorySet {
    repositories: HashMap<PathBuf, Option<Arc<Snapshot>>>,
    folders: Arc<HashMap<PathBuf, FolderSummary>>,
}

/// Built once on the status worker. Hovering a folder never scans its files.
#[derive(Clone, Debug)]
pub struct FolderSummary {
    pub status: FileStatus,
    pub changed: usize,
    pub deleted: usize,
    /// A deterministic example, shared by all its ancestor folders.
    pub example: Arc<PathBuf>,
}

impl RepositorySet {
    pub fn for_roots(roots: &[PathBuf], previous: &Self) -> Result<Self, String> {
        let mut result = Self {
            repositories: roots
                .iter()
                .map(|root| {
                    (
                        root.clone(),
                        previous.repositories.get(root).cloned().flatten(),
                    )
                })
                .collect(),
            folders: previous.folders.clone(),
        };
        // New boundaries can mask paths formerly reported by a parent repo.
        if roots.len() != previous.repositories.len()
            || roots
                .iter()
                .any(|root| !previous.repositories.contains_key(root))
        {
            result.folders = Arc::new(result.aggregate_folders()?);
        }
        Ok(result)
    }
    pub fn status(&self, path: &Path) -> Option<FileStatus> {
        for ancestor in path.ancestors() {
            if let Some(snapshot) = self.repositories.get(ancestor) {
                return snapshot.as_ref()?.status(path);
            }
        }
        None
    }
    pub fn repository(&self, root: &Path) -> Option<&Arc<Snapshot>> {
        self.repositories.get(root)?.as_ref()
    }
    pub fn folder_status(&self, path: &Path) -> Option<FileStatus> {
        self.folder_summary(path).map(|summary| summary.status)
    }
    pub fn folder_summary(&self, path: &Path) -> Option<&FolderSummary> {
        self.folders.get(path)
    }
    pub fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.repositories.keys()
    }
    pub fn watch_directories(&self) -> impl Iterator<Item = &PathBuf> {
        self.repositories
            .values()
            .flatten()
            .flat_map(|snapshot| &snapshot.watch_directories)
    }
    /// Called on a worker: equality and aggregate limits may visit many paths.
    /// Returns false for an unchanged snapshot, and retains old data on error.
    pub fn update(
        &mut self,
        root: &Path,
        snapshot: Option<Snapshot>,
        limits: Limits,
    ) -> Result<bool, String> {
        if self
            .repositories
            .get(root)
            .and_then(|value| value.as_deref())
            == snapshot.as_ref()
        {
            return Ok(false);
        }
        let mut paths = 0;
        let mut bytes = 0;
        for value in self
            .repositories
            .iter()
            .filter(|(key, _)| key.as_path() != root)
            .filter_map(|(_, value)| value.as_deref())
            .chain(snapshot.as_ref())
        {
            paths += value.files.len();
            bytes += value
                .files
                .keys()
                .map(|path| path.as_os_str().len())
                .sum::<usize>();
        }
        if paths > limits.changed_paths || bytes > limits.path_bytes {
            return Err("Git status exceeds the window's changed-file limit".into());
        }
        let old = self
            .repositories
            .insert(root.to_owned(), snapshot.map(Arc::new));
        match self.aggregate_folders() {
            Ok(folders) => self.folders = Arc::new(folders),
            Err(error) => {
                if let Some(old) = old {
                    self.repositories.insert(root.to_owned(), old);
                } else {
                    self.repositories.remove(root);
                }
                return Err(error);
            }
        }
        Ok(true)
    }

    fn aggregate_folders(&self) -> Result<HashMap<PathBuf, FolderSummary>, String> {
        let mut folders = HashMap::<PathBuf, FolderSummary>::new();
        let mut bytes = 0;
        for (root, snapshot) in &self.repositories {
            let Some(snapshot) = snapshot else { continue };
            for (relative, status) in &snapshot.files {
                let path = root.join(relative);
                let is_repo_entry = self.repositories.contains_key(&path);
                let nearest = path
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                    .find(|ancestor| self.repositories.contains_key(*ancestor));
                if nearest != Some(root.as_path()) {
                    continue;
                }
                let first = if is_repo_entry {
                    Some(path.as_path())
                } else {
                    path.parent()
                };
                let mut example = None;
                for parent in first.into_iter().flat_map(Path::ancestors) {
                    if let Some(entry) = folders.get_mut(parent) {
                        entry.status.merge(*status);
                        entry.changed += 1;
                        entry.deleted += usize::from(status.deleted);
                        if path < *entry.example {
                            entry.example = example
                                .get_or_insert_with(|| Arc::new(path.clone()))
                                .clone();
                        }
                    } else {
                        bytes += parent.as_os_str().len();
                        if folders.len() >= 100_000 || bytes > 16 * 1024 * 1024 {
                            return Err("Git folder status exceeds the summary limit".into());
                        }
                        folders.insert(
                            parent.to_owned(),
                            FolderSummary {
                                status: *status,
                                changed: 1,
                                deleted: usize::from(status.deleted),
                                example: example
                                    .get_or_insert_with(|| Arc::new(path.clone()))
                                    .clone(),
                            },
                        );
                    }
                }
            }
        }
        Ok(folders)
    }
}

/// Discover only requested directories (expanded folders and open-file parents).
/// No descendant walk or status computation. Roots are deduplicated and ordered.
pub fn discover(directories: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<PathBuf>, String> {
    let mut roots = BTreeSet::new();
    for directory in directories {
        if cancel.load(Ordering::Relaxed) {
            return Err("Git discovery cancelled".into());
        }
        match gix::discover(directory) {
            Ok(repo) => {
                if let Some(root) = repo.workdir() {
                    roots.insert(root.to_owned());
                }
            }
            Err(error) if error.is_not_found() => {}
            Err(error) => return Err(error.to_string()),
        }
        if roots.len() > 64 {
            return Err("Git discovery exceeds the 64-repository limit".into());
        }
    }
    Ok(roots.into_iter().collect())
}

impl Snapshot {
    pub fn status(&self, path: &Path) -> Option<FileStatus> {
        self.files.get(path.strip_prefix(&self.root).ok()?).copied()
    }
    pub fn changed_files(&self) -> usize {
        self.files.len()
    }
}

/// Bounds retained results, not the repository's transient Git index. A limit
/// never produces a partial snapshot that could make changed files look clean.
#[derive(Clone, Copy)]
pub struct Limits {
    pub changed_paths: usize,
    pub path_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            changed_paths: 100_000,
            path_bytes: 16 * 1024 * 1024,
        }
    }
}

/// `None` means no working-tree repository. Does not write the index/config,
/// invoke Git, run filters/hooks, or access the network. Cancellation is
/// cooperative; callers retain their previous snapshot if this returns Err.
pub fn status(
    directory: &Path,
    cancel: Arc<AtomicBool>,
    limits: Limits,
) -> Result<Option<Snapshot>, String> {
    let cancelled = || {
        if cancel.load(Ordering::Relaxed) {
            Err("Git status cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    cancelled()?;
    let mut repo = match gix::discover(directory) {
        Ok(repo) => repo,
        Err(error) if error.is_not_found() => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let Some(root) = repo.workdir().map(Path::to_owned) else {
        return Ok(None);
    };
    // Native EOL/encoding conversions remain available. Executable filters and
    // external diff drivers are excluded from this in-memory configuration.
    {
        let mut config = repo.config_snapshot_mut();
        let ids: Vec<_> = ["filter", "diff"]
            .into_iter()
            .flat_map(|name| {
                config
                    .sections_and_ids_by_name(name)
                    .into_iter()
                    .flatten()
                    .map(|(_, id)| id)
            })
            .collect();
        for id in ids {
            config.remove_section_by_id(id);
        }
        config.commit().map_err(|error| error.to_string())?;
    }
    let common = std::fs::canonicalize(repo.common_dir()).map_err(|e| e.to_string())?;
    let git_dir = std::fs::canonicalize(repo.git_dir()).map_err(|e| e.to_string())?;
    let mut watch_directories = vec![git_dir.clone(), common.clone()];
    if let Some(name) = repo.head_name().map_err(|error| error.to_string())? {
        let path = common.join(gix::path::from_bstr(name.as_bstr()).map_err(|e| e.to_string())?);
        if let Some(parent) = path.parent() {
            watch_directories.push(parent.to_owned());
        }
    }
    watch_directories.sort();
    watch_directories.dedup();
    let index = repo.index_or_empty().map_err(|e| e.to_string())?;
    let watch = watch::Plan::from_index(&index, &root, &[git_dir, common], &cancel)?;
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| e.to_string())?
        .index(index.into())
        .untracked_files(gix::status::UntrackedFiles::None)
        .index_worktree_submodules(None)
        .index_worktree_rewrites(None)
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .index_worktree_options_mut(|options| options.thread_limit = Some(2))
        .should_interrupt_owned(cancel.clone())
        .into_iter(Vec::new())
        .map_err(|e| e.to_string())?;
    let mut files = HashMap::<PathBuf, FileStatus>::new();
    let mut path_bytes = 0;
    for item in iter {
        cancelled()?;
        let item = item.map_err(|e| e.to_string())?;
        let (staged, modified, conflict, deleted) = match &item {
            gix::status::Item::TreeIndex(change) => (
                true,
                false,
                false,
                matches!(change, gix::diff::index::Change::Deletion { .. }),
            ),
            gix::status::Item::IndexWorktree(change) => {
                let Some(summary) = change.summary() else {
                    continue;
                };
                (
                    false,
                    true,
                    summary
                        == gix::status::plumbing::index_as_worktree_with_renames::Summary::Conflict,
                    summary
                        == gix::status::plumbing::index_as_worktree_with_renames::Summary::Removed,
                )
            }
        };
        let path = gix::path::from_bstr(item.location()).map_err(|e| e.to_string())?;
        if !files.contains_key(path.as_ref()) {
            path_bytes += item.location().len();
            if files.len() >= limits.changed_paths || path_bytes > limits.path_bytes {
                cancel.store(true, Ordering::Relaxed);
                return Err("Git status exceeds the changed-file limit".into());
            }
        }
        let entry = files.entry(path.into_owned()).or_default();
        entry.staged |= staged;
        entry.modified |= modified;
        entry.conflict |= conflict;
        entry.deleted |= deleted;
    }
    cancelled()?;
    Ok(Some(Snapshot {
        root,
        watch_directories,
        watch: Some(Arc::new(watch)),
        files,
    }))
}

#[cfg(test)]
mod tests;
