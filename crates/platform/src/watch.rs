//! Parent-directory watches survive atomic file replacement. Registration and
//! teardown run on a worker, with bounded/coalesced delivery to the UI.
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

#[derive(Default)]
pub struct Batch {
    pub events: Vec<Event>,
    pub rescan: bool,
}
impl Batch {
    /// Namespace changes affect a directory's immediate listing. Plain file
    /// writes do not require enumerating that directory again.
    pub fn affects_directory(&self, directory: &std::path::Path) -> bool {
        self.rescan
            || self.events.iter().any(|event| {
                !matches!(
                    event.kind,
                    EventKind::Access(_)
                        | EventKind::Modify(notify::event::ModifyKind::Data(_))
                        | EventKind::Modify(notify::event::ModifyKind::Metadata(_))
                ) && event
                    .paths
                    .iter()
                    .any(|path| path.parent() == Some(directory) || directory.starts_with(path))
            })
    }
    pub fn ignore_rules_changed(&self) -> bool {
        self.events
            .iter()
            .flat_map(|event| &event.paths)
            .any(|path| {
                path.file_name()
                    .is_some_and(|name| name == ".gitignore" || name == ".ignore")
            })
    }
    pub fn renames(&self) -> impl Iterator<Item = (&std::path::Path, &std::path::Path)> {
        self.events
            .iter()
            .filter(|event| {
                (matches!(
                    event.kind,
                    EventKind::Modify(notify::event::ModifyKind::Name(
                        notify::event::RenameMode::Both
                    ))
                ) && event.paths.len() == 2)
            })
            .map(|event| (event.paths[0].as_path(), event.paths[1].as_path()))
    }
    pub fn affects(&self, path: &std::path::Path) -> bool {
        self.rescan
            || self
                .events
                .iter()
                .any(|event| event.paths.iter().any(|p| path == p || path.starts_with(p)))
    }
}

pub struct FileWatches {
    paths: mpsc::Sender<WatchPaths>,
    pending: Arc<Mutex<Batch>>,
}

struct WatchPaths {
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
}

impl FileWatches {
    pub fn new() -> (Self, async_channel::Receiver<()>) {
        let (wake, events) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(Batch::default()));
        let (paths, incoming) = mpsc::channel::<WatchPaths>();
        let output = pending.clone();
        std::thread::Builder::new()
            .name("file-watches".into())
            .spawn(move || {
                let changed = output.clone();
                let notify = wake.clone();
                let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
                    let mut batch = changed.lock().unwrap();
                    match event {
                        Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                            batch.rescan |= event.need_rescan();
                            if batch.events.len() < 256 {
                                batch.events.push(event);
                            } else {
                                batch.rescan = true;
                            }
                        }
                        Ok(_) => return,
                        Err(_) => batch.rescan = true,
                    }
                    let _ = notify.try_send(());
                });
                let mut watcher = watcher.ok();
                let mut watched = HashSet::new();
                while let Ok(mut paths) = incoming.recv() {
                    for newer in incoming.try_iter() {
                        paths = newer;
                    }
                    let parents = paths.files.into_iter().flat_map(|path| {
                        [Some(path.clone()), std::fs::canonicalize(path).ok()]
                            .into_iter()
                            .flatten()
                            .filter_map(|path| path.parent().map(std::path::Path::to_owned))
                    });
                    let mut directories = HashSet::new();
                    for directory in parents.chain(paths.directories) {
                        if let Some(parent) = directory.ancestors().find(|path| path.is_dir()) {
                            directories.insert(
                                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_owned()),
                            );
                        }
                    }
                    if let Some(watcher) = &mut watcher {
                        for directory in watched.difference(&directories) {
                            let _ = watcher.unwatch(directory);
                        }
                        for directory in directories.difference(&watched) {
                            let _ = watcher.watch(directory, RecursiveMode::NonRecursive);
                        }
                    }
                    watched = directories;
                    // Recheck after registration to cover the interval while watches changed.
                    output.lock().unwrap().rescan = true;
                    let _ = wake.try_send(());
                }
            })
            .expect("start file watcher");
        (Self { paths, pending }, events)
    }
    pub fn set_paths(&self, paths: Vec<PathBuf>) {
        self.set_paths_and_directories(paths, Vec::new());
    }
    pub fn set_paths_and_directories(&self, files: Vec<PathBuf>, directories: Vec<PathBuf>) {
        let _ = self.paths.send(WatchPaths { files, directories });
    }
    pub fn take(&self) -> Batch {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RenameMode};
    use std::time::{Duration, Instant};

    #[test]
    fn directory_batches_target_listings_and_ignore_plain_writes() {
        let root = PathBuf::from("project");
        let nested = root.join("nested");
        let event = |kind, paths| Batch {
            events: vec![Event {
                kind,
                paths,
                ..Event::default()
            }],
            rescan: false,
        };
        let created = event(
            EventKind::Create(CreateKind::File),
            vec![nested.join("new.txt")],
        );
        assert!(created.affects_directory(&nested));
        assert!(!created.affects_directory(&root));
        let renamed = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            vec![nested.clone(), root.join("renamed")],
        );
        assert!(renamed.affects_directory(&root));
        assert!(renamed.affects_directory(&nested));
        assert!(renamed.affects_directory(&nested.join("child")));
        let written = event(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            vec![nested.join("file.txt")],
        );
        assert!(!written.affects_directory(&nested));
        let ignore = event(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            vec![root.join(".gitignore")],
        );
        assert!(ignore.ignore_rules_changed());
        assert!(
            Batch {
                rescan: true,
                ..Batch::default()
            }
            .affects_directory(&nested)
        );
    }

    #[test]
    fn directory_watch_observes_create_rename_and_delete_without_open_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (watches, events) = FileWatches::new();
        watches.set_paths_and_directories(Vec::new(), vec![root.clone()]);
        let wait = |predicate: &dyn Fn(&Batch) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if events.try_recv().is_ok() && predicate(&watches.take()) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "directory watcher did not deliver an update"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(&|batch| batch.rescan);
        let old = root.join("old.txt");
        let new = root.join("new.txt");
        std::fs::write(&old, "content").unwrap();
        wait(&|batch| {
            batch.affects_directory(&root)
                && batch
                    .events
                    .iter()
                    .any(|e| matches!(e.kind, EventKind::Create(_)) && e.paths.contains(&old))
        });
        std::fs::rename(&old, &new).unwrap();
        wait(&|batch| {
            batch.affects_directory(&root) && batch.events.iter().any(|e| e.paths.contains(&new))
        });
        std::fs::remove_file(&new).unwrap();
        wait(&|batch| {
            batch.affects_directory(&root)
                && batch
                    .events
                    .iter()
                    .any(|e| matches!(e.kind, EventKind::Remove(_)) && e.paths.contains(&new))
        });
    }

    #[test]
    fn parent_watch_observes_writes_and_atomic_replacements() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path())
            .unwrap()
            .join("document.txt");
        std::fs::write(&path, "first").unwrap();
        let (watches, events) = FileWatches::new();
        watches.set_paths(vec![path.clone()]);
        let wait = |registered: bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if events.try_recv().is_ok() {
                    let batch = watches.take();
                    if registered && batch.rescan || !registered && batch.affects(&path) {
                        return;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "file watcher did not deliver an update"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(true);
        std::fs::write(&path, "second").unwrap();
        wait(false);
        let replacement = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        std::fs::write(replacement.path(), "replacement").unwrap();
        replacement.persist(&path).unwrap();
        wait(false);
        std::fs::write(&path, "after replacement").unwrap();
        wait(false);
    }
}
