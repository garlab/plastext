use super::*;
use plastext_git::{Decoration, DiscoveryCache, FileStatus, RepositorySet};
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

#[derive(Default)]
struct Pending {
    all: bool,
    paths: std::collections::HashSet<PathBuf>,
}

pub(super) struct State {
    pub snapshot: Option<Arc<RepositorySet>>,
    pub refreshing: bool,
    pub failed: bool,
    scopes: Vec<PathBuf>,
    tree_scopes: Vec<PathBuf>,
    tree_revision: Option<u64>,
    tab_scopes: Vec<PathBuf>,
    generation: u64,
    pending: Pending,
    wake: async_channel::Sender<()>,
    cancel: Arc<AtomicBool>,
    _task: gpui::Task<()>,
    #[cfg(test)]
    scan_counts: HashMap<PathBuf, usize>,
}

impl State {
    pub fn new(scope: Option<PathBuf>, cx: &mut Context<Workspace>) -> Self {
        let (wake, events) = async_channel::bounded(1);
        let task = cx.spawn(async move |this, cx| {
            let mut first = true;
            let mut discovery = DiscoveryCache::default();
            let mut schedule = Schedule::default();
            let mut active_roots = Vec::new();
            let mut next_scan = None;
            loop {
                if !first {
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
                first = false;
                while events.try_recv().is_ok() {}
                let job = this.update(cx, |this, _| {
                    (
                        this.git.scopes.clone(),
                        this.git.generation,
                        std::mem::take(&mut this.git.pending),
                        this.git.snapshot.clone(),
                    )
                });
                let Ok((scopes, generation, pending, previous)) = job else {
                    break;
                };
                if scopes.is_empty() {
                    if events.recv().await.is_err() {
                        break;
                    }
                    first = true;
                    continue;
                }
                for path in &pending.paths {
                    if path.file_name().is_some_and(|name| name == ".git")
                        && let Some(parent) = path.parent()
                    {
                        discovery.invalidate(parent);
                    }
                }
                let now = cx.background_executor().now();
                let mut discovery_failed = false;
                if discovery.needs_refresh(&scopes, now) {
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
                }
                match discovery.cached(&scopes) {
                    Ok(roots) if !discovery_failed => active_roots = roots,
                    _ => discovery_failed = true,
                }
                let roots = schedule.reconcile(&active_roots, now);
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
                let due = if next_scan.is_none_or(|deadline| now >= deadline) {
                    schedule.due(now)
                } else {
                    Vec::new()
                };
                let mut scan_time = Duration::ZERO;
                for root in due {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let previous = this
                        .update(cx, |this, cx| {
                            if this.git.generation != generation {
                                return None;
                            }
                            this.git.cancel = cancel.clone();
                            this.git.refreshing = true;
                            #[cfg(test)]
                            {
                                *this.git.scan_counts.entry(root.clone()).or_default() += 1;
                            }
                            cx.notify();
                            this.git.snapshot.clone()
                        })
                        .ok()
                        .flatten();
                    let Some(previous) = previous else { break };
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
                    next_scan = Some(cx.background_executor().now() + cooldown(scan_time));
                }
                if this
                    .update(cx, |this, cx| {
                        let failed = discovery_failed || schedule.failed();
                        if this.git.refreshing || this.git.failed != failed {
                            this.git.refreshing = false;
                            this.git.failed = failed;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            snapshot: None,
            refreshing: false,
            failed: false,
            scopes: scope.into_iter().collect(),
            tree_scopes: Vec::new(),
            tree_revision: None,
            tab_scopes: Vec::new(),
            generation: 0,
            pending: Pending::default(),
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
    #[cfg(test)]
    pub fn request(&mut self) {
        self.pending.all = true;
        self.wake();
    }
    pub fn observe(&mut self, batch: &plastext_platform::watch::Batch) {
        // Watch registration emits an empty rescan hint. It must not turn
        // expanding/collapsing a folder into a full Git status scan.
        for path in batch.events.iter().flat_map(|event| &event.paths) {
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
    pub fn directories(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.snapshot
            .iter()
            .filter(|_| !self.scopes.is_empty())
            .flat_map(|set| set.watch_directories().chain(set.roots()).cloned())
    }
    pub fn label(&self) -> Option<&'static str> {
        if self.failed {
            Some("Git: some status unavailable")
        } else if self.refreshing {
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
        if !tree_changed && tab_scopes == self.git.tab_scopes {
            return;
        }
        self.git.tab_scopes = tab_scopes;
        let mut scopes = self.git.tree_scopes.clone();
        scopes.extend(self.root.iter().cloned());
        scopes.extend(self.git.tab_scopes.iter().cloned());
        scopes.sort();
        scopes.dedup();
        if scopes != self.git.scopes {
            self.git.scopes = scopes;
            self.git.generation = self.git.generation.wrapping_add(1);
            self.git.wake();
        }
    }
}
