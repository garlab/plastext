use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(60);
const MAX_DIRECTORIES: usize = 4096;

#[derive(Default)]
pub struct DiscoveryCache {
    entries: HashMap<PathBuf, Entry>,
}
struct Entry {
    checked: Instant,
    result: Result<Option<PathBuf>, String>,
}

impl DiscoveryCache {
    /// Working repository watches invalidate status when a repository moves or
    /// loses its Git directory; don't rediscover those same roots on a timer.
    pub fn retain_watched(&mut self, roots: &std::collections::HashSet<PathBuf>, now: Instant) {
        for entry in self.entries.values_mut() {
            if entry
                .result
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|root| roots.contains(root))
            {
                entry.checked = now;
            }
        }
    }
    pub fn invalidate(&mut self, directory: &Path) {
        self.entries.retain(|path, _| !path.starts_with(directory));
    }
    pub fn needs_refresh(&self, directories: &[PathBuf], now: Instant) -> bool {
        directories.iter().any(|path| {
            self.entries
                .get(path)
                .is_none_or(|entry| now.saturating_duration_since(entry.checked) >= TTL)
        })
    }
    /// Cache both positive and negative discovery, never recurse into children.
    /// Call on a worker when needs_refresh() is true; cached() is I/O-free.
    pub fn refresh(&mut self, directories: &[PathBuf], now: Instant, cancel: &AtomicBool) {
        for directory in directories.iter().take(MAX_DIRECTORIES) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if !self.needs_refresh(std::slice::from_ref(directory), now) {
                continue;
            }
            let result = match gix::discover(directory) {
                Ok(repo) => Ok(repo.workdir().map(Path::to_owned)),
                Err(error) if error.is_not_found() => Ok(None),
                Err(error) => Err(error.to_string()),
            };
            if self.entries.len() >= MAX_DIRECTORIES && !self.entries.contains_key(directory) {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.checked)
                    .map(|(path, _)| path.clone());
                if let Some(oldest) = oldest {
                    self.entries.remove(&oldest);
                }
            }
            self.entries.insert(
                directory.clone(),
                Entry {
                    checked: now,
                    result,
                },
            );
        }
    }
    pub fn cached(&self, directories: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        if directories.len() > MAX_DIRECTORIES {
            return Err("Git discovery exceeds the directory limit".into());
        }
        let mut roots = BTreeSet::new();
        for directory in directories {
            if let Some(entry) = self.entries.get(directory)
                && let Some(root) = entry.result.as_ref().map_err(Clone::clone)?
            {
                roots.insert(root.clone());
            }
        }
        if roots.len() > 64 {
            return Err("Git discovery exceeds the 64-repository limit".into());
        }
        Ok(roots.into_iter().collect())
    }
}
