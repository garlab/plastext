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
    paths: mpsc::Sender<Vec<PathBuf>>,
    pending: Arc<Mutex<Batch>>,
}

impl FileWatches {
    pub fn new() -> (Self, async_channel::Receiver<()>) {
        let (wake, events) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(Batch::default()));
        let (paths, incoming) = mpsc::channel::<Vec<PathBuf>>();
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
                    let mut directories = HashSet::new();
                    for path in paths {
                        for path in [Some(path.clone()), std::fs::canonicalize(path).ok()]
                            .into_iter()
                            .flatten()
                        {
                            if let Some(parent) = path.parent() {
                                let existing = parent.ancestors().find(|path| path.is_dir());
                                if let Some(parent) = existing {
                                    directories.insert(parent.to_owned());
                                }
                            }
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
        let _ = self.paths.send(paths);
    }
    pub fn take(&self) -> Batch {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
