use super::*;
use plastext_git::{Decoration, DiscoveryCache, FileStatus, RepositorySet, git_trace};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(crate) fn color(status: Option<FileStatus>, theme: Theme) -> Option<gpui::Rgba> {
    Some(match status?.decoration()? {
        Decoration::Staged => theme.git_staged,
        Decoration::Modified => theme.git_modified,
        Decoration::Mixed => theme.git_mixed,
        Decoration::Conflict => theme.git_conflict,
    })
}

mod schedule;
use schedule::Schedule;

#[derive(Debug, Default)]
struct Pending {
    all: bool,
    paths: std::collections::HashSet<PathBuf>,
    repositories: std::collections::HashSet<PathBuf>,
}

pub(super) struct State {
    pub snapshot: Option<Arc<RepositorySet>>,
    refreshing: bool,
    refresh_visible: bool,
    refresh_indicator: Option<gpui::Task<()>>,
    pub failed: bool,
    scopes: Vec<PathBuf>,
    tree_scopes: Vec<PathBuf>,
    tree_priorities: Vec<PathBuf>,
    priorities: Vec<PathBuf>,
    tree_revision: Option<u64>,
    tab_scopes: Vec<PathBuf>,
    generation: u64,
    pending: Pending,
    healthy_watches: std::collections::HashSet<PathBuf>,
    watches: Option<plastext_git::watch::Watches>,
    enabled: bool,
    _watch_events: gpui::Task<()>,
    wake: async_channel::Sender<()>,
    cancel: Arc<AtomicBool>,
    _task: gpui::Task<()>,
    #[cfg(test)]
    scan_counts: HashMap<PathBuf, usize>,
}

impl State {
    pub fn new(scope: Option<PathBuf>, cx: &mut Context<Workspace>) -> Self {
        git_trace!("workspace_start scope={scope:?}");
        let (wake, events) = async_channel::bounded(1);
        // Native watcher behavior has its own integration tests. GPUI's
        // deterministic clock tests inject the same Changes messages directly.
        #[cfg(not(test))]
        let (watches, watch_events) = {
            let (watches, events) = plastext_git::watch::Watches::new();
            (Some(watches), events)
        };
        #[cfg(test)]
        let (watches, watch_events) = (None, async_channel::bounded::<()>(1).1);
        let watch_task = cx.spawn(async move |this, cx| {
            while watch_events.recv().await.is_ok() {
                if this
                    .update(cx, |this, _| {
                        let changes = this.git.watches.as_ref().unwrap().take();
                        this.git.observe_watches(changes);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let mut immediate = true;
            let mut discovery = DiscoveryCache::default();
            let mut schedule = Schedule::default();
            let mut active_roots = Vec::new();
            let mut next_scan = None;
            let mut last_tick = std::time::SystemTime::now();
            loop {
                if !immediate {
                    let event = events.recv();
                    // This tick only checks deadlines in memory. Disk work is scheduled
                    // per repository, and discovery has a separate positive/negative cache.
                    let poll = cx.background_executor().timer(Duration::from_secs(1));
                    futures::pin_mut!(event, poll);
                    match futures::future::select(event, poll).await {
                        futures::future::Either::Left((Err(_), _)) => break,
                        futures::future::Either::Left((Ok(()), _)) => {
                            cx.background_executor()
                                .timer(Duration::from_millis(300))
                                .await;
                        }
                        futures::future::Either::Right(_) => {}
                    }
                }
                while events.try_recv().is_ok() {}
                let job = this.update(cx, |this, _| {
                    // A long wall-clock gap also detects sleep/resume when the
                    // active window doesn't receive a new activation event.
                    let now = std::time::SystemTime::now();
                    if now
                        .duration_since(last_tick)
                        .is_ok_and(|elapsed| elapsed > Duration::from_secs(30))
                    {
                        git_trace!(
                            "invalidate_all reason=clock_gap elapsed={:?}",
                            now.duration_since(last_tick)
                        );
                        this.git.pending.all = true;
                    }
                    last_tick = now;
                    (
                        this.git.scopes.clone(),
                        this.git.priorities.clone(),
                        this.git.generation,
                        if this.git.enabled {
                            std::mem::take(&mut this.git.pending)
                        } else {
                            Pending::default()
                        },
                        this.git.snapshot.clone(),
                        this.git.healthy_watches.clone(),
                        this.git.enabled,
                    )
                });
                let Ok((scopes, priorities, generation, pending, previous, healthy, enabled)) = job
                else {
                    break;
                };
                if scopes.is_empty() || !enabled {
                    if events.recv().await.is_err() {
                        break;
                    }
                    immediate = true;
                    continue;
                }
                if pending.all || !pending.paths.is_empty() || !pending.repositories.is_empty() {
                    git_trace!("schedule_pending {pending:?}");
                }
                for path in &pending.paths {
                    if path.file_name().is_some_and(|name| name == ".git")
                        && let Some(parent) = path.parent()
                    {
                        discovery.invalidate(parent);
                    }
                }
                let now = cx.background_executor().now();
                discovery.retain_watched(&healthy, now);
                let mut discovery_failed = false;
                if discovery.needs_refresh(&scopes, now) {
                    git_trace!("discovery_start scopes={}", scopes.len());
                    let cancel = Arc::new(AtomicBool::new(false));
                    if this
                        .update(cx, |this, _| this.git.cancel = cancel.clone())
                        .is_err()
                    {
                        break;
                    }
                    let paths = scopes.clone();
                    let flag = cancel.clone();
                    let mut cache = std::mem::take(&mut discovery);
                    let work = cx.background_executor().spawn(async move {
                        cache.refresh(&paths, now, &flag);
                        Ok(cache)
                    });
                    match bounded(work, &cancel, cx).await {
                        Ok(cache) => discovery = cache,
                        Err(_) => discovery_failed = true,
                    }
                    git_trace!("discovery_end failed={discovery_failed}");
                }
                match discovery.cached(&scopes) {
                    Ok(roots) if !discovery_failed => active_roots = roots,
                    _ => discovery_failed = true,
                }
                let roots = schedule.reconcile(&active_roots, now);
                schedule.set_healthy(&healthy);
                schedule.prioritize(&priorities, now);
                schedule.invalidate(&pending, previous.as_deref());
                let mut old_roots: Vec<_> = previous
                    .iter()
                    .flat_map(|set| set.roots().cloned())
                    .collect();
                old_roots.sort();
                if roots != old_roots || previous.is_none() {
                    let work = cx.background_executor().spawn(async move {
                        RepositorySet::for_roots(
                            &roots,
                            previous.as_deref().unwrap_or(&RepositorySet::default()),
                        )
                        .map(Arc::new)
                    });
                    match work.await {
                        Ok(snapshot) => {
                            if this
                                .update(cx, |this, cx| this.publish_git(snapshot, cx))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => discovery_failed = true,
                    }
                }
                let now = cx.background_executor().now();
                let mut due = schedule.due(now);
                if next_scan.is_some_and(|deadline| now < deadline) {
                    // The repository being browsed shouldn't inherit a slow,
                    // unrelated repository's cooldown. Its own cooldown holds.
                    due.retain(|root| schedule.is_priority(root));
                }
                let mut scan_time = Duration::ZERO;
                for root in due {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let previous = this
                        .update(cx, |this, cx| {
                            if this.git.generation != generation || !this.git.enabled {
                                return None;
                            }
                            this.git.cancel = cancel.clone();
                            this.git.start_refresh(cx);
                            #[cfg(test)]
                            {
                                *this.git.scan_counts.entry(root.clone()).or_default() += 1;
                            }
                            this.git.snapshot.clone()
                        })
                        .ok()
                        .flatten();
                    let Some(previous) = previous else { break };
                    git_trace!(
                        "scan_start root={root:?} reason={} priority={}",
                        schedule.reason(&root),
                        schedule.is_priority(&root)
                    );
                    schedule.started(&root);
                    let flag = cancel.clone();
                    let path = root.clone();
                    let started = Instant::now();
                    let work = cx.background_executor().spawn(async move {
                        let snapshot = plastext_git::status(&path, flag, Default::default())?
                            .filter(|snapshot| snapshot.root == path);
                        let gone = snapshot.is_none();
                        let mut next = (*previous).clone();
                        next.update(&path, snapshot, Default::default())
                            .map(|changed| (gone, changed.then(|| Arc::new(next))))
                    });
                    let result = bounded(work, &cancel, cx).await;
                    git_trace!(
                        "scan_end root={root:?} elapsed_ms={} result={:?}",
                        started.elapsed().as_millis(),
                        result.as_ref().map(|(gone, snapshot)| if *gone {
                            "gone"
                        } else if snapshot.is_some() {
                            "changed"
                        } else {
                            "unchanged"
                        })
                    );
                    scan_time += started.elapsed();
                    schedule.completed(
                        &root,
                        cx.background_executor().now(),
                        started.elapsed(),
                        result.is_err(),
                    );
                    if let Ok((gone, snapshot)) = result {
                        if gone {
                            schedule.remove(&root);
                            discovery.invalidate(&root);
                        }
                        if let Some(snapshot) = snapshot
                            && this
                                .update(cx, |this, cx| this.publish_git(snapshot, cx))
                                .is_err()
                        {
                            break;
                        }
                    }
                    // Finish the current repository, then yield storage time.
                    // Unvisited roots stay due for the next paced batch.
                    if scan_time >= Duration::from_millis(500) {
                        break;
                    }
                }
                if !scan_time.is_zero() {
                    // A window containing many repositories must not keep the
                    // storage busy just because each individual repo is cheap.
                    let deadline = cx.background_executor().now() + cooldown(scan_time);
                    // A quick priority scan must not shorten the breathing
                    // room owed to earlier, more expensive background work.
                    next_scan = Some(next_scan.map_or(deadline, |old| old.max(deadline)));
                }
                match this.update(cx, |this, cx| {
                    let failed = discovery_failed || schedule.failed();
                    this.git.finish_refresh(failed, cx);
                    this.git.generation != generation
                }) {
                    // Re-evaluate changed browsing priorities before waiting
                    // on the event debounce or the background polling tick.
                    Ok(changed) => immediate = changed,
                    Err(_) => break,
                }
            }
        });
        Self {
            snapshot: None,
            refreshing: false,
            refresh_visible: false,
            refresh_indicator: None,
            failed: false,
            scopes: scope.iter().cloned().collect(),
            tree_scopes: Vec::new(),
            tree_priorities: Vec::new(),
            priorities: scope.into_iter().collect(),
            tree_revision: None,
            tab_scopes: Vec::new(),
            generation: 0,
            pending: Pending::default(),
            healthy_watches: Default::default(),
            watches,
            enabled: true,
            _watch_events: watch_task,
            wake,
            cancel: Arc::new(AtomicBool::new(false)),
            _task: task,
            #[cfg(test)]
            scan_counts: HashMap::new(),
        }
    }
    fn wake(&self) {
        let _ = self.wake.try_send(());
    }
    fn start_refresh(&mut self, cx: &mut Context<Workspace>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        // One delay for the whole batch, including consecutive small scans.
        // Finishing the batch drops this task, so quick checks never flash.
        self.refresh_indicator = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            this.update(cx, |this, cx| {
                if this.git.refreshing {
                    this.git.refresh_visible = true;
                    cx.notify();
                }
            })
            .ok();
        }));
    }
    fn finish_refresh(&mut self, failed: bool, cx: &mut Context<Workspace>) {
        self.refreshing = false;
        self.refresh_indicator = None;
        let visible = std::mem::take(&mut self.refresh_visible);
        if visible || self.failed != failed {
            self.failed = failed;
            cx.notify();
        }
    }
    pub fn set_active(&mut self, active: bool) {
        if self.enabled != active {
            git_trace!("activation active={active}");
            self.enabled = active;
            // Focus restoration is a reconciliation point for missed events,
            // external tools and watch-less/network filesystems.
            if active {
                self.pending.all = true;
            }
            self.wake();
        }
    }
    fn observe_watches(&mut self, changes: plastext_git::watch::Changes) {
        git_trace!(
            "watch_batch dirty={:?} healthy={:?}",
            changes.dirty,
            changes.healthy
        );
        self.pending.repositories.extend(changes.dirty);
        if let Some(healthy) = changes.healthy {
            self.healthy_watches = healthy;
        }
        self.wake();
    }
    pub fn changed_file(&mut self, path: &Path) {
        git_trace!("document_changed path={path:?}");
        // Existing open-document metadata checks can detect changes even on a
        // network filesystem whose native watch silently emits no events.
        if self.pending.paths.len() < 256 {
            self.pending.paths.insert(path.to_owned());
        } else {
            self.pending.all = true;
        }
        self.wake();
    }
    #[cfg(test)]
    pub fn request(&mut self) {
        self.pending.all = true;
        self.wake();
    }
    pub fn observe(&mut self, batch: &plastext_platform::watch::Batch) {
        // Watch registration emits an empty rescan hint. It must not turn
        // expanding/collapsing a folder into a full Git status scan.
        for path in batch.events.iter().flat_map(|event| &event.paths) {
            // Git's own watcher filters build output before delivery. Tree
            // watches remain useful for discovering new repositories and for
            // incomplete watch coverage, but must not bypass that filter.
            let relevant = path.file_name().is_some_and(|name| name == ".git")
                || self.snapshot.as_ref().is_some_and(|set| {
                    set.roots().any(|root| {
                        if self.healthy_watches.contains(root) {
                            return false;
                        }
                        set.repository(root)
                            .and_then(|repo| repo.watch.as_ref())
                            .is_some_and(|plan| plan.relevant(path))
                    })
                });
            if !relevant {
                continue;
            }
            git_trace!("tree_invalidate path={path:?}");
            if self.pending.paths.len() < 256 {
                self.pending.paths.insert(path.clone());
            } else {
                self.pending.all = true;
            }
        }
        if !batch.events.is_empty() {
            self.wake();
        }
    }
    fn update_watches(&self) {
        if let Some(watches) = &self.watches {
            let plans = self
                .snapshot
                .iter()
                .filter(|_| !self.scopes.is_empty())
                .flat_map(|set| {
                    set.roots()
                        .filter_map(|root| set.repository(root)?.watch.clone())
                })
                .collect();
            watches.set(plans);
        }
    }
    pub fn label(&self) -> Option<&'static str> {
        if self.failed {
            Some("Git: some status unavailable")
        } else if self.refresh_visible {
            Some("Git: refreshing…")
        } else {
            None
        }
    }
}

async fn bounded<T>(
    work: gpui::Task<Result<T, String>>,
    cancel: &AtomicBool,
    cx: &gpui::AsyncApp,
) -> Result<T, String> {
    let timeout = cx.background_executor().timer(Duration::from_secs(10));
    futures::pin_mut!(work, timeout);
    match futures::future::select(work, timeout).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right((_, work)) => {
            cancel.store(true, Ordering::Relaxed);
            // Never overlap scans, even when cooperative shutdown is delayed.
            let _ = work.await;
            Err("Git status took too long".into())
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn cooldown(elapsed: Duration) -> Duration {
    elapsed
        .saturating_mul(4)
        .clamp(Duration::from_secs(1), Duration::from_secs(60))
}

#[cfg(test)]
mod tests;

impl Workspace {
    fn publish_git(&mut self, snapshot: Arc<RepositorySet>, cx: &mut Context<Self>) {
        let retired = self.git.snapshot.replace(snapshot.clone());
        if let Some(tree) = &self.tree {
            tree.update(cx, |tree, cx| tree.set_git_status(Some(snapshot), cx));
        }
        self.update_disk_watches(cx);
        self.git.update_watches();
        cx.notify();
        cx.background_executor()
            .spawn(async move { drop(retired) })
            .detach();
    }

    pub(super) fn update_git_scope(&mut self, cx: &mut Context<Self>) {
        let revision = self.tree.as_ref().map(|tree| tree.read(cx).git_revision());
        let tree_changed = revision != self.git.tree_revision;
        if tree_changed {
            self.git.tree_scopes = self
                .tree
                .as_ref()
                .map(|tree| tree.read(cx).git_discovery_directories())
                .unwrap_or_default();
            self.git.tree_revision = revision;
            self.git.tree_priorities = self
                .tree
                .as_ref()
                .map(|tree| tree.read(cx).watched_directories().to_vec())
                .unwrap_or_default();
        }
        let mut tab_scopes = Vec::new();
        for tab in self.panes.values().flat_map(|pane| &pane.tabs) {
            if tab
                .view
                .editing_document(cx)
                .is_some_and(|doc| doc.read(cx).is_untitled())
            {
                continue;
            }
            tab_scopes.extend(tab.path.parent().map(Path::to_owned));
        }
        tab_scopes.sort();
        tab_scopes.dedup();
        let mut priorities = self.git.tree_priorities.clone();
        priorities.extend(tab_scopes.iter().cloned());
        priorities.extend(self.tree.as_ref().and_then(|tree| tree.read(cx).selected()));
        priorities.extend(self.root.iter().cloned());
        priorities.sort();
        priorities.dedup();
        let priority_changed = priorities != self.git.priorities;
        if !tree_changed && tab_scopes == self.git.tab_scopes && !priority_changed {
            return;
        }
        self.git.tab_scopes = tab_scopes;
        let mut scopes = self.git.tree_scopes.clone();
        scopes.extend(self.root.iter().cloned());
        scopes.extend(self.git.tab_scopes.iter().cloned());
        scopes.sort();
        scopes.dedup();
        if scopes != self.git.scopes || priority_changed {
            git_trace!(
                "scope_changed scopes={} priorities={priorities:?}",
                scopes.len()
            );
            let was_empty = self.git.scopes.is_empty();
            self.git.scopes = scopes;
            self.git.priorities = priorities;
            self.git.generation = self.git.generation.wrapping_add(1);
            self.git.wake();
            if was_empty != self.git.scopes.is_empty() {
                self.git.update_watches();
            }
        }
    }
}
