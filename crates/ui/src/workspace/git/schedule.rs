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
    priority: bool,
    dirty: bool,
    next_event: Instant,
    next_poll: Instant,
    checked: Instant,
    failed: bool,
    healthy: bool,
}
impl Entry {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(if self.healthy && !self.failed {
            600
        } else if self.priority {
            30
        } else {
            120
        })
    }
}
impl Schedule {
    pub fn reconcile(&mut self, roots: &[PathBuf], now: Instant) -> Vec<PathBuf> {
        self.usage += 1;
        for (root, entry) in &mut self.entries {
            entry.active = roots.contains(root);
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
                    priority: false,
                    dirty: true,
                    next_event: now,
                    next_poll: now,
                    checked: now,
                    failed: false,
                    healthy: false,
                },
            );
        }
        self.roots()
    }
    pub fn set_healthy(&mut self, healthy: &std::collections::HashSet<PathBuf>) {
        for (root, entry) in &mut self.entries {
            let covered = healthy.contains(root);
            if entry.healthy != covered {
                entry.healthy = covered;
                entry.next_poll = entry.checked + entry.poll_interval();
            }
        }
    }
    /// Expanded folders and open files come first. Re-entering a fresh cache
    /// does no work; an older cache is revalidated without waiting behind others.
    pub fn prioritize(&mut self, paths: &[PathBuf], now: Instant) {
        let roots: std::collections::HashSet<_> = paths
            .iter()
            .filter_map(|path| {
                path.ancestors()
                    .find(|parent| self.entries.contains_key(*parent))
                    .map(Path::to_owned)
            })
            .collect();
        for (root, entry) in &mut self.entries {
            let priority = roots.contains(root);
            if priority != entry.priority {
                entry.priority = priority;
                entry.next_poll = entry.checked + entry.poll_interval();
                if priority {
                    entry.next_event = entry.next_event.min(entry.checked + Duration::from_secs(5));
                    entry.dirty |= !entry.healthy
                        && now.saturating_duration_since(entry.checked) >= Duration::from_secs(5);
                }
            }
        }
    }
    pub fn is_priority(&self, root: &Path) -> bool {
        self.entries.get(root).is_some_and(|entry| entry.priority)
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
                || pending.repositories.contains(root)
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
        entries.sort_by_key(|(path, e)| (!e.priority, !e.dirty, !e.active, e.next_poll, *path));
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
            let wait = if entry.priority {
                cooldown(elapsed).min(Duration::from_secs(5))
            } else {
                cooldown(elapsed)
            };
            entry.next_event = now + wait;
            entry.next_poll = now + entry.poll_interval();
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
        schedule.prioritize(&roots, now);
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
                ..Default::default()
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

    #[test]
    fn browsing_prioritizes_missing_or_stale_status_without_rescanning_fresh_caches() {
        let now = Instant::now();
        let roots = [PathBuf::from("a-background"), PathBuf::from("z-browsing")];
        let mut schedule = Schedule::default();
        schedule.reconcile(&roots, now);
        schedule.prioritize(&[roots[1].join("src")], now);
        assert_eq!(schedule.due(now), [roots[1].clone(), roots[0].clone()]);
        for root in &roots {
            schedule.started(root);
            schedule.completed(root, now, Duration::from_millis(10), false);
        }
        schedule.prioritize(&[], now);
        schedule.prioritize(&[roots[1].clone()], now + Duration::from_secs(2));
        assert!(schedule.due(now + Duration::from_secs(2)).is_empty());
        // Merely listed repositories no longer all rescan every 30 seconds.
        assert_eq!(
            schedule.due(now + Duration::from_secs(30)),
            [roots[1].clone()]
        );
        schedule.prioritize(&[], now + Duration::from_secs(30));
        assert!(schedule.due(now + Duration::from_secs(30)).is_empty());
        // Navigating back to an older cache queues it immediately.
        schedule.prioritize(&[roots[1].clone()], now + Duration::from_secs(31));
        assert_eq!(
            schedule.due(now + Duration::from_secs(31)),
            [roots[1].clone()]
        );
        assert!(schedule.is_priority(&roots[1]));
        assert!(!schedule.is_priority(&roots[0]));
    }

    #[test]
    fn healthy_watches_keep_idle_repositories_quiet_and_failures_restore_fallback() {
        let now = Instant::now();
        let root = PathBuf::from("repository");
        let mut schedule = Schedule::default();
        schedule.reconcile(std::slice::from_ref(&root), now);
        schedule.set_healthy(&[root.clone()].into());
        schedule.started(&root);
        schedule.completed(&root, now, Duration::from_millis(10), false);
        // Re-entering the repository and the former 30s/120s polling deadlines
        // cause no scan while event coverage is healthy.
        schedule.prioritize(std::slice::from_ref(&root), now + Duration::from_secs(10));
        for seconds in [30, 120, 599] {
            assert!(schedule.due(now + Duration::from_secs(seconds)).is_empty());
        }
        assert_eq!(
            schedule.due(now + Duration::from_secs(600)),
            std::slice::from_ref(&root)
        );
        schedule.invalidate(
            &Pending {
                all: false,
                paths: [root.clone()].into(),
                ..Default::default()
            },
            None,
        );
        assert_eq!(
            schedule.due(now + Duration::from_secs(10)),
            std::slice::from_ref(&root)
        );
        schedule.started(&root);
        schedule.completed(
            &root,
            now + Duration::from_secs(10),
            Duration::from_millis(10),
            false,
        );
        schedule.set_healthy(&Default::default());
        assert_eq!(schedule.due(now + Duration::from_secs(40)), [root]);
    }
}
