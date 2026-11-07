use super::*;
use plastext_git::{Decoration, FileStatus, RepositorySet};
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

pub(super) struct State {
    pub snapshot: Option<Arc<RepositorySet>>,
    pub refreshing: bool,
    pub failed: bool,
    scopes: Vec<PathBuf>,
    generation: u64,
    wake: async_channel::Sender<()>,
    cancel: Arc<AtomicBool>,
    _task: gpui::Task<()>,
}

impl State {
    pub fn new(scope: Option<PathBuf>, cx: &mut Context<Workspace>) -> Self {
        let (wake, events) = async_channel::bounded(1);
        let task = cx.spawn(async move |this, cx| {
            let mut first = true;
            let mut elapsed = Duration::ZERO;
            let mut last_generation = 0;
            loop {
                if !first {
                    let cooling = cx.background_executor().timer(cooldown(elapsed));
                    let event = events.recv();
                    let poll = cx
                        .background_executor()
                        .timer(cooldown(elapsed) + Duration::from_secs(10));
                    futures::pin_mut!(event, poll);
                    match futures::future::select(event, poll).await {
                        futures::future::Either::Left((Err(_), _)) => break,
                        futures::future::Either::Left((Ok(()), _)) => {
                            let Ok(generation) = this.read_with(cx, |this, _| this.git.generation)
                            else {
                                break;
                            };
                            if generation == last_generation {
                                cooling.await;
                            }
                            cx.background_executor()
                                .timer(Duration::from_millis(300))
                                .await;
                        }
                        futures::future::Either::Right(_) => {}
                    }
                }
                while events.try_recv().is_ok() {}
                let cancel = Arc::new(AtomicBool::new(false));
                let job = match this.update(cx, |this, cx| {
                    if this.git.scopes.is_empty() {
                        return None;
                    }
                    this.git.cancel = cancel.clone();
                    this.git.refreshing = true;
                    cx.notify();
                    Some((
                        this.git.scopes.clone(),
                        this.git.generation,
                        this.git.snapshot.clone(),
                    ))
                }) {
                    Ok(job) => job,
                    Err(_) => break,
                };
                let Some((scopes, generation, previous)) = job else {
                    if events.recv().await.is_err() {
                        break;
                    }
                    first = true;
                    continue;
                };
                first = false;
                last_generation = generation;
                let started = Instant::now();
                let flag = cancel.clone();
                let discovery = cx.background_executor().spawn(async move {
                    let roots = plastext_git::discover(&scopes, &flag)?;
                    let snapshot = RepositorySet::for_roots(
                        &roots,
                        previous.as_deref().unwrap_or(&RepositorySet::default()),
                    );
                    Ok((roots, Arc::new(snapshot)))
                });
                let mut failed = false;
                match bounded(discovery, &cancel, cx).await {
                    Ok((roots, snapshot)) => {
                        if this
                            .update(cx, |this, cx| {
                                if this.git.generation == generation {
                                    this.publish_git(snapshot, cx);
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                        // Publish each completed repository immediately. A slow or broken
                        // repository keeps its old colors without blocking healthy ones.
                        for root in roots {
                            let cancel = Arc::new(AtomicBool::new(false));
                            let previous = this
                                .update(cx, |this, _| {
                                    if this.git.generation != generation {
                                        return None;
                                    }
                                    this.git.cancel = cancel.clone();
                                    this.git.snapshot.clone()
                                })
                                .ok()
                                .flatten();
                            let Some(previous) = previous else { break };
                            let flag = cancel.clone();
                            let work = cx.background_executor().spawn(async move {
                                let snapshot =
                                    plastext_git::status(&root, flag, Default::default())?;
                                let mut next = (*previous).clone();
                                next.update(&root, snapshot, Default::default())
                                    .map(|changed| changed.then(|| Arc::new(next)))
                            });
                            match bounded(work, &cancel, cx).await {
                                Ok(Some(snapshot)) => {
                                    if this
                                        .update(cx, |this, cx| {
                                            if this.git.generation == generation {
                                                this.publish_git(snapshot, cx);
                                            }
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                Ok(None) => {}
                                Err(_) => failed = true,
                            }
                        }
                    }
                    Err(_) => failed = true,
                }
                elapsed = started.elapsed();
                if this
                    .update(cx, |this, cx| {
                        this.git.refreshing = false;
                        if this.git.generation == generation {
                            this.git.failed = failed;
                        }
                        cx.notify();
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
            generation: 0,
            wake,
            cancel: Arc::new(AtomicBool::new(false)),
            _task: task,
        }
    }
    pub fn request(&self) {
        let _ = self.wake.try_send(());
    }
    pub fn directories(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.snapshot
            .iter()
            .flat_map(|snapshot| snapshot.watch_directories().cloned())
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
        let mut scopes = self
            .tree
            .as_ref()
            .map(|tree| tree.read(cx).watched_directories().to_vec())
            .unwrap_or_default();
        scopes.extend(self.root.iter().cloned());
        for tab in self.panes.values().flat_map(|pane| &pane.tabs) {
            if tab
                .view
                .editing_document(cx)
                .is_some_and(|doc| doc.read(cx).is_untitled())
            {
                continue;
            }
            scopes.extend(tab.path.parent().map(Path::to_owned));
        }
        scopes.sort();
        scopes.dedup();
        if scopes != self.git.scopes {
            self.git.cancel.store(true, Ordering::Relaxed);
            self.git.scopes = scopes;
            self.git.generation = self.git.generation.wrapping_add(1);
            self.git.failed = false;
            self.git.request();
            if self.git.scopes.is_empty() {
                let retired = self.git.snapshot.take();
                if let Some(tree) = &self.tree {
                    tree.update(cx, |tree, cx| tree.set_git_status(None, cx));
                }
                cx.background_executor()
                    .spawn(async move { drop(retired) })
                    .detach();
            }
        }
    }
}
