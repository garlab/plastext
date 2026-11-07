use super::*;
use plastext_git::{Decoration, FileStatus, Snapshot};
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
    pub snapshot: Option<Arc<Snapshot>>,
    pub refreshing: bool,
    pub failed: bool,
    scope: Option<PathBuf>,
    generation: u64,
    wake: async_channel::Sender<()>,
    cancel: Arc<AtomicBool>,
    _task: gpui::Task<()>,
}

impl State {
    pub fn new(scope: Option<PathBuf>, cx: &mut Context<Workspace>) -> Self {
        let (wake, events) = async_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let task = cx.spawn(async move |this, cx| {
            let mut first = true;
            let mut elapsed = Duration::ZERO;
            let mut last_generation = 0;
            loop {
                if !first {
                    // Slow repositories back off, including during event storms.
                    // Changing scope skips that backoff, but never overlaps scans.
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
                    let directory = this.git.scope.clone()?;
                    this.git.cancel = cancel.clone();
                    this.git.refreshing = true;
                    cx.notify();
                    Some((directory, this.git.generation, this.git.snapshot.clone()))
                }) {
                    Ok(job) => job,
                    Err(_) => break,
                };
                let Some((path, generation, previous)) = job else {
                    if events.recv().await.is_err() {
                        break;
                    }
                    first = true;
                    continue;
                };
                first = false;
                last_generation = generation;
                let flag = cancel.clone();
                let started = Instant::now();
                let work = cx.background_executor().spawn(async move {
                    plastext_git::status(&path, flag, Default::default()).map(|snapshot| {
                        // Equality can visit many changed paths; keep it off the UI thread.
                        let changed = previous.as_deref() != snapshot.as_ref();
                        (changed, snapshot.map(Arc::new))
                    })
                });
                let timeout = cx.background_executor().timer(Duration::from_secs(10));
                futures::pin_mut!(work, timeout);
                let result = match futures::future::select(work, timeout).await {
                    futures::future::Either::Left((result, _)) => result,
                    futures::future::Either::Right((_, work)) => {
                        cancel.store(true, Ordering::Relaxed);
                        // Await cooperative shutdown before allowing another scan.
                        let _ = work.await;
                        Err("Git status took too long".into())
                    }
                };
                elapsed = started.elapsed();
                let retired = this.update(cx, |this, cx| {
                    this.git.refreshing = false;
                    if this.git.generation != generation {
                        return result.ok().and_then(|(_, snapshot)| snapshot);
                    }
                    this.git.failed = result.is_err();
                    let mut retired = None;
                    if let Ok((changed, snapshot)) = result {
                        if changed {
                            retired = std::mem::replace(&mut this.git.snapshot, snapshot);
                            if let Some(tree) = &this.tree {
                                tree.update(cx, |tree, cx| {
                                    tree.set_git_status(this.git.snapshot.clone(), cx)
                                });
                            }
                            this.update_disk_watches(cx);
                        } else {
                            retired = snapshot;
                        }
                    }
                    cx.notify();
                    retired
                });
                match retired {
                    Ok(retired) => cx
                        .background_executor()
                        .spawn(async move { drop(retired) })
                        .detach(),
                    Err(_) => break,
                }
            }
        });
        Self {
            snapshot: None,
            refreshing: false,
            failed: false,
            scope,
            generation: 0,
            wake,
            cancel,
            _task: task,
        }
    }
    pub fn request(&self) {
        let _ = self.wake.try_send(());
    }
    pub fn directories(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.snapshot
            .iter()
            .flat_map(|snapshot| snapshot.watch_directories.iter().cloned())
    }
    pub fn label(&self) -> Option<&'static str> {
        if self.failed {
            Some("Git: status unavailable")
        } else if self.refreshing {
            Some("Git: refreshing…")
        } else {
            None
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
    pub(super) fn update_git_scope(&mut self, cx: &mut Context<Self>) {
        // A folderless window follows its active saved file's repository.
        let scope = self.root.clone().or_else(|| {
            let tab = self.panes[&self.active_pane].active_tab()?;
            if tab
                .view
                .editing_document(cx)
                .is_some_and(|doc| doc.read(cx).is_untitled())
            {
                return None;
            }
            if self
                .git
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| tab.path.starts_with(&snapshot.root))
            {
                return self.git.scope.clone();
            }
            tab.path.parent().map(Path::to_owned)
        });
        if scope != self.git.scope {
            let retired = self.git.snapshot.take();
            self.git.cancel.store(true, Ordering::Relaxed);
            self.git.scope = scope;
            self.git.generation = self.git.generation.wrapping_add(1);
            self.git.failed = false;
            self.git.request();
            if let Some(tree) = &self.tree {
                tree.update(cx, |tree, cx| tree.set_git_status(None, cx));
            }
            cx.background_executor()
                .spawn(async move { drop(retired) })
                .detach();
        }
    }
}
