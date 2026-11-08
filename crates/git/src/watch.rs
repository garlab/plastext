//! Repository event filtering and bounded native watch registration.
//! No Git work, filesystem inspection, or UI callbacks run in the event handler.
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

mod service;
pub use service::Watches;

const MAX_DIRECTORIES: usize = 8192;
const MAX_PATH_BYTES: usize = 512 * 1024;
const MAX_NATIVE_WATCHES: usize = 4096;
const NATIVE_RECURSION: bool = cfg!(any(target_os = "macos", target_os = "windows"));

#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub root: PathBuf,
    metadata: Vec<PathBuf>,
    /// Relative directories containing tracked files, including ancestors.
    /// None means the bounded index exceeded its limit: retain polling fallback.
    directories: Option<HashSet<PathBuf>>,
}

impl Plan {
    pub(crate) fn from_index(
        index: &gix::index::State,
        root: &Path,
        metadata: &[PathBuf],
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let mut directories = HashSet::from([PathBuf::new()]);
        let mut bytes = 0;
        let mut complete = true;
        for entry in index.entries() {
            if cancel.load(Ordering::Relaxed) {
                return Err("Git status cancelled".into());
            }
            let path = gix::path::from_bstr(entry.path(index)).map_err(|e| e.to_string())?;
            for parent in path.parent().into_iter().flat_map(Path::ancestors) {
                if directories.contains(parent) {
                    break;
                }
                bytes += parent.as_os_str().len();
                if directories.len() >= MAX_DIRECTORIES || bytes > MAX_PATH_BYTES {
                    complete = false;
                    break;
                }
                directories.insert(parent.to_owned());
            }
            if !complete {
                break;
            }
        }
        // A linked worktree's private directory can be inside the common
        // directory. Match it first so its index/HEAD aren't treated as noise.
        let mut metadata = metadata.to_vec();
        metadata.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        metadata.dedup();
        Ok(Self {
            root: root.to_owned(),
            metadata,
            directories: complete.then_some(directories),
        })
    }

    pub fn relevant(&self, path: &Path) -> bool {
        if self.root.starts_with(path) {
            return true;
        }
        for directory in &self.metadata {
            if directory.starts_with(path) {
                return true;
            }
            if let Ok(relative) = path.strip_prefix(directory) {
                // Object writes and lock churn do not change status. The final
                // index/ref rename does, and refs cover packed/unpacked branches.
                if relative
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".lock"))
                {
                    return false;
                }
                return relative.components().next().is_some_and(|part| {
                    matches!(
                        part.as_os_str().to_str(),
                        Some(
                            "HEAD"
                                | "index"
                                | "packed-refs"
                                | "config"
                                | "config.worktree"
                                | "commondir"
                                | "shallow"
                                | "refs"
                                | "info"
                        )
                    )
                });
            }
        }
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        if relative == Path::new(".git") {
            return true;
        }
        self.directories.as_ref().is_none_or(|dirs| {
            dirs.contains(relative)
                || relative
                    .parent()
                    .is_some_and(|parent| dirs.contains(parent))
        })
    }

    fn paths(&self) -> (Vec<(PathBuf, RecursiveMode)>, bool) {
        let mut complete = self.directories.is_some();
        let recursive = NATIVE_RECURSION && self.directories.is_some();
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        let mut paths = vec![(self.root.clone(), mode)];
        if let Some(parent) = self.root.parent() {
            paths.push((parent.to_owned(), RecursiveMode::NonRecursive));
        }
        for directory in &self.metadata {
            paths.push((directory.clone(), mode));
            if !recursive {
                // Enumerate only Git refs, never the worktree/build outputs.
                let mut stack = vec![directory.join("refs")];
                while let Some(path) = stack.pop() {
                    if paths.len() >= MAX_NATIVE_WATCHES {
                        complete = false;
                        break;
                    }
                    let children = match std::fs::read_dir(&path) {
                        Ok(children) => children,
                        Err(error) => {
                            complete &= error.kind() == std::io::ErrorKind::NotFound;
                            continue;
                        }
                    };
                    paths.push((path, RecursiveMode::NonRecursive));
                    for (count, child) in children.enumerate() {
                        if count >= MAX_NATIVE_WATCHES {
                            complete = false;
                            break;
                        }
                        match child.and_then(|child| Ok((child.file_type()?, child.path()))) {
                            Ok((kind, path)) if kind.is_dir() => stack.push(path),
                            Ok(_) => {}
                            Err(_) => complete = false,
                        }
                        if stack.len() >= MAX_NATIVE_WATCHES {
                            complete = false;
                            break;
                        }
                    }
                }
                let info = directory.join("info");
                if info.is_dir() {
                    paths.push((info, RecursiveMode::NonRecursive));
                }
            }
        }
        if !recursive && let Some(dirs) = &self.directories {
            let mut dirs: Vec<_> = dirs
                .iter()
                .filter(|dir| !dir.as_os_str().is_empty())
                .collect();
            dirs.sort();
            paths.extend(
                dirs.into_iter()
                    .map(|dir| (self.root.join(dir), RecursiveMode::NonRecursive)),
            );
        }
        (paths, complete)
    }
}

/// Events are coalesced by repository, not queued by file. An event storm has
/// the same bounded memory cost as a single change in each of the 64 caches.
#[derive(Default)]
pub struct Changes {
    pub dirty: HashSet<PathBuf>,
    pub healthy: Option<HashSet<PathBuf>>,
}

#[derive(Default)]
struct Shared {
    plans: Vec<Arc<Plan>>,
    healthy: HashSet<PathBuf>,
    changes: Changes,
    registered: HashSet<PathBuf>,
    reset: bool,
    generation: u64,
}
impl Shared {
    fn event(&mut self, event: notify::Result<Event>) -> bool {
        let event = match event {
            Ok(event) if matches!(event.kind, EventKind::Access(_)) => return false,
            Ok(event) if !event.need_rescan() => event,
            _ => {
                self.changes
                    .dirty
                    .extend(self.plans.iter().map(|plan| plan.root.clone()));
                self.healthy.clear();
                self.changes.healthy = Some(HashSet::new());
                self.reset = true;
                self.generation = self.generation.wrapping_add(1);
                return true;
            }
        };
        // Renaming/removing a watched directory can detach a watch from its
        // path. Reinstall it instead of claiming coverage for the old inode.
        if matches!(
            event.kind,
            EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
        ) && event
            .paths
            .iter()
            .any(|path| self.registered.contains(path))
        {
            self.reset = true;
            self.generation = self.generation.wrapping_add(1);
            self.healthy.clear();
            self.changes.healthy = Some(HashSet::new());
        }
        if !NATIVE_RECURSION
            && matches!(
                event.kind,
                EventKind::Create(notify::event::CreateKind::Folder)
            )
            && event.paths.iter().any(|path| {
                self.plans.iter().any(|plan| {
                    plan.metadata
                        .iter()
                        .any(|dir| path.starts_with(dir.join("refs")))
                })
            })
        {
            self.reset = true;
            self.generation = self.generation.wrapping_add(1);
            self.healthy.clear();
            self.changes.healthy = Some(HashSet::new());
        }
        let mut relevant = false;
        for plan in &self.plans {
            if event.paths.iter().any(|path| plan.relevant(path)) {
                self.changes.dirty.insert(plan.root.clone());
                relevant = true;
            }
        }
        relevant
    }
}

#[cfg(test)]
mod tests;
