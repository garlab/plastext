//! Read-only, UI-independent Git status. Call on a background worker.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileStatus {
    pub staged: bool,
    pub modified: bool,
    pub conflict: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoration {
    Staged,
    Modified,
    Mixed,
    Conflict,
}

impl FileStatus {
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
    files: HashMap<PathBuf, FileStatus>,
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
    let mut watch_directories = vec![
        std::fs::canonicalize(repo.git_dir()).map_err(|e| e.to_string())?,
        common.clone(),
    ];
    if let Some(name) = repo.head_name().map_err(|error| error.to_string())? {
        let path = common.join(gix::path::from_bstr(name.as_bstr()).map_err(|e| e.to_string())?);
        if let Some(parent) = path.parent() {
            watch_directories.push(parent.to_owned());
        }
    }
    watch_directories.sort();
    watch_directories.dedup();
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| e.to_string())?
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
        let (staged, modified, conflict) = match &item {
            gix::status::Item::TreeIndex(_) => (true, false, false),
            gix::status::Item::IndexWorktree(change) => {
                let Some(summary) = change.summary() else {
                    continue;
                };
                (
                    false,
                    true,
                    summary
                        == gix::status::plumbing::index_as_worktree_with_renames::Summary::Conflict,
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
    }
    cancelled()?;
    Ok(Some(Snapshot {
        root,
        watch_directories,
        files,
    }))
}

#[cfg(test)]
mod tests;
