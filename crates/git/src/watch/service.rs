use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Watch setup, retries and teardown stay on one worker. Replacing a snapshot
/// doesn't restart unchanged watches. Dropping the sender stops the worker.
pub struct Watches {
    plans: mpsc::Sender<Vec<Arc<Plan>>>,
    shared: Arc<Mutex<Shared>>,
}
impl Watches {
    pub fn new() -> (Self, async_channel::Receiver<()>) {
        let (wake, events) = async_channel::bounded(1);
        let (plans, incoming) = mpsc::channel::<Vec<Arc<Plan>>>();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let state = shared.clone();
        std::thread::Builder::new()
            .name("git-watches".into())
            .spawn(move || {
                let mut watcher = None;
                let mut watched = BTreeMap::<PathBuf, RecursiveMode>::new();
                let mut current = Vec::<Arc<Plan>>::new();
                let mut retry = Instant::now();
                loop {
                    let received = incoming.recv_timeout(Duration::from_secs(5));
                    let mut next = match received {
                        Ok(plans) => plans,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if Instant::now() < retry || current.is_empty() {
                                continue;
                            }
                            let state = state.lock().unwrap();
                            if state.healthy.len() == current.len() {
                                continue;
                            }
                            current.clone()
                        }
                    };
                    for plans in incoming.try_iter() {
                        next = plans;
                    }
                    next.sort_by(|a, b| a.root.cmp(&b.root));
                    next.truncate(64);
                    if next == current {
                        let healthy = state.lock().unwrap().healthy.len() == current.len();
                        if healthy || Instant::now() < retry {
                            continue;
                        }
                    }
                    let changed: HashSet<_> = next
                        .iter()
                        .filter(|plan| !current.contains(plan))
                        .map(|plan| plan.root.clone())
                        .collect();
                    current = next;
                    retry = Instant::now() + Duration::from_secs(60);
                    let generation = {
                        let mut state = state.lock().unwrap();
                        if state.reset {
                            // Drop outside the callback's mutex: native teardown
                            // can wait for a callback which uses that mutex.
                            watched.clear();
                        }
                        state.plans = current.clone();
                        state
                            .changes
                            .dirty
                            .retain(|root| current.iter().any(|plan| plan.root == *root));
                        state
                            .healthy
                            .retain(|root| current.iter().any(|plan| plan.root == *root));
                        (state.generation, std::mem::take(&mut state.reset))
                    };
                    if generation.1 {
                        watcher = None;
                    }
                    if watcher.is_none() {
                        let output = state.clone();
                        let wake = wake.clone();
                        watcher = notify::recommended_watcher(move |event| {
                            if output.lock().unwrap().event(event) {
                                let _ = wake.try_send(());
                            }
                        })
                        .ok();
                    }
                    let mut desired = BTreeMap::new();
                    let mut required = Vec::new();
                    for plan in &current {
                        let (paths, complete) = plan.paths();
                        for (path, mode) in &paths {
                            if desired.len() < MAX_NATIVE_WATCHES || desired.contains_key(path) {
                                desired
                                    .entry(path.clone())
                                    .and_modify(|old| {
                                        if *mode == RecursiveMode::Recursive {
                                            *old = *mode;
                                        }
                                    })
                                    .or_insert(*mode);
                            }
                        }
                        required.push((paths, complete));
                    }
                    let mut setup_failed = false;
                    let registration_changed = desired != watched;
                    if registration_changed && let Some(watcher) = &mut watcher {
                        let mut edits = watcher.paths_mut();
                        let obsolete: Vec<_> = watched
                            .iter()
                            .filter(|(path, mode)| desired.get(*path) != Some(*mode))
                            .map(|(path, _)| path.clone())
                            .collect();
                        for path in obsolete {
                            let _ = edits.remove(&path);
                            watched.remove(&path);
                        }
                        for (path, mode) in &desired {
                            if !watched.contains_key(path) && edits.add(path, *mode).is_ok() {
                                watched.insert(path.clone(), *mode);
                            }
                        }
                        if edits.commit().is_err() {
                            watched.clear();
                            setup_failed = true;
                        }
                    }
                    if setup_failed {
                        watcher = None;
                    }
                    let healthy: HashSet<_> = current
                        .iter()
                        .zip(&required)
                        .filter(|(plan, (paths, complete))| {
                            plan.directories.is_some()
                                && *complete
                                && paths.iter().all(|(path, _)| watched.contains_key(path))
                        })
                        .map(|(plan, _)| plan.root.clone())
                        .collect();
                    {
                        let mut state = state.lock().unwrap();
                        let healthy = if state.generation == generation.0 {
                            healthy
                        } else {
                            HashSet::new()
                        };
                        // Cover the gap between the status snapshot and registration,
                        // and reconcile when failed watches recover.
                        let recovered: Vec<_> =
                            healthy.difference(&state.healthy).cloned().collect();
                        state.changes.dirty.extend(changed);
                        state.changes.dirty.extend(recovered);
                        if cfg!(target_os = "macos") && registration_changed {
                            // notify restarts its FSEvents stream for path edits.
                            // Reconcile all scopes across that small delivery gap.
                            state
                                .changes
                                .dirty
                                .extend(current.iter().map(|plan| plan.root.clone()));
                        }
                        state.healthy = healthy.clone();
                        state.changes.healthy = Some(healthy);
                        state.registered = watched.keys().cloned().collect();
                    }
                    let _ = wake.try_send(());
                }
            })
            .expect("start Git watcher");
        (Self { plans, shared }, events)
    }
    pub fn set(&self, plans: Vec<Arc<Plan>>) {
        let _ = self.plans.send(plans);
    }
    pub fn take(&self) -> Changes {
        std::mem::take(&mut self.shared.lock().unwrap().changes)
    }
}
