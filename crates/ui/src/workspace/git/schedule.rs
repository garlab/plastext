use super::*;

const MAX_REPOS: usize = 64;

#[derive(Default)]
pub(super) struct Schedule {
    entries: HashMap<PathBuf, Entry>,
    usage: u64,
}
struct Entry {
    used: u64,
    active: bool,
    dirty: bool,
    next_event: Instant,
    next_poll: Instant,
    checked: Instant,
    failed: bool,
}
impl Schedule {
    pub fn reconcile(&mut self, roots: &[PathBuf], now: Instant) -> Vec<PathBuf> {
        self.usage += 1;
        for (root, entry) in &mut self.entries {
            let was_active = entry.active;
            entry.active = roots.contains(root);
            if entry.active != was_active {
                entry.next_poll =
                    entry.checked + Duration::from_secs(if entry.active { 30 } else { 120 });
            }
            if entry.active {
                entry.used = self.usage;
            }
        }
        for root in roots {
            if self.entries.contains_key(root) {
                continue;
            }
            if self.entries.len() == MAX_REPOS {
                let oldest = self
                    .entries
                    .iter()
                    .filter(|(_, e)| !e.active)
                    .min_by_key(|(path, e)| (e.used, *path))
                    .map(|(path, _)| path.clone());
                if let Some(oldest) = oldest {
                    self.entries.remove(&oldest);
                } else {
                    break;
                }
            }
            self.entries.insert(
                root.clone(),
                Entry {
                    used: self.usage,
                    active: true,
                    dirty: true,
                    next_event: now,
                    next_poll: now,
                    checked: now,
                    failed: false,
                },
            );
        }
        self.roots()
    }
    pub fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<_> = self.entries.keys().cloned().collect();
        roots.sort();
        roots
    }
    pub fn remove(&mut self, root: &Path) {
        self.entries.remove(root);
    }
    pub fn invalidate(&mut self, pending: &Pending, snapshot: Option<&RepositorySet>) {
        for (root, entry) in &mut self.entries {
            if pending.all
                || pending.paths.iter().any(|path| {
                    path.starts_with(root)
                        || root.starts_with(path)
                        || snapshot
                            .and_then(|set| set.repository(root))
                            .is_some_and(|snapshot| {
                                snapshot
                                    .watch_directories
                                    .iter()
                                    .any(|dir| path.starts_with(dir))
                            })
                })
            {
                entry.dirty = true;
            }
        }
    }
    pub fn due(&self, now: Instant) -> Vec<PathBuf> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| now >= e.next_event && (e.dirty || now >= e.next_poll))
            .collect();
        entries.sort_by_key(|(path, e)| (!e.active, e.next_poll, *path));
        entries.into_iter().map(|(path, _)| path.clone()).collect()
    }
    pub fn started(&mut self, root: &Path) {
        if let Some(entry) = self.entries.get_mut(root) {
            entry.dirty = false;
        }
    }
    pub fn completed(&mut self, root: &Path, now: Instant, elapsed: Duration, failed: bool) {
        if let Some(entry) = self.entries.get_mut(root) {
            entry.failed = failed;
            entry.checked = now;
            entry.next_event = now + cooldown(elapsed);
            entry.next_poll = now + Duration::from_secs(if entry.active { 30 } else { 120 });
        }
    }
    pub fn failed(&self) -> bool {
        self.entries.values().any(|e| e.failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_cached_repositories_does_not_rescan_and_events_target_one_repo() {
        let now = Instant::now();
        let roots = [PathBuf::from("projects/a"), PathBuf::from("projects/b")];
        let mut schedule = Schedule::default();
        schedule.reconcile(&roots, now);
        for root in &roots {
            schedule.started(root);
            schedule.completed(root, now, Duration::from_millis(20), false);
        }
        assert!(schedule.due(now + Duration::from_secs(2)).is_empty());
        assert_eq!(schedule.reconcile(&[], now).len(), 2);
        schedule.reconcile(&roots, now + Duration::from_secs(3));
        assert!(schedule.due(now + Duration::from_secs(3)).is_empty());
        schedule.invalidate(
            &Pending {
                all: false,
                paths: [roots[0].join("src/file")].into(),
            },
            None,
        );
        assert!(schedule.due(now + Duration::from_millis(500)).is_empty());
        assert_eq!(
            schedule.due(now + Duration::from_secs(4)),
            [roots[0].clone()]
        );
        assert_eq!(schedule.due(now + Duration::from_secs(30)).len(), 2);
    }

    #[test]
    fn cache_evicts_the_least_recently_used_inactive_repository() {
        let now = Instant::now();
        let roots: Vec<_> = (0..64)
            .map(|i| PathBuf::from(format!("repo{i:02}")))
            .collect();
        let mut schedule = Schedule::default();
        schedule.reconcile(&roots, now);
        schedule.reconcile(std::slice::from_ref(&roots[0]), now);
        let new = PathBuf::from("new");
        let retained = schedule.reconcile(&[roots[0].clone(), new.clone()], now);
        assert_eq!(retained.len(), 64);
        assert!(retained.contains(&roots[0]));
        assert!(retained.contains(&new));
        assert!(!retained.contains(&roots[1]));
    }
}
