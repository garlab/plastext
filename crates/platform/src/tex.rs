//! Automatic offline compilation, active only while a TeX preview is visible.
mod compiler;
mod discovery;
mod snapshot;

pub use crate::pdf::Cancel;
pub use compiler::{Compiled, compile};
use notify::{RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const CANCELLED: &str = "TeX build cancelled";
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}
fn check(cancel: &Cancel) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub executable: PathBuf,
    installation: PathBuf,
    font_cache: Option<PathBuf>,
}
#[derive(Clone)]
pub struct Config {
    pub root: PathBuf,
    pub input: PathBuf,
    pub engine: Engine,
    pub input_folders: Vec<PathBuf>,
    base: PathBuf,
    automatic: Option<PathBuf>,
    extra_inputs: Vec<PathBuf>,
}
impl Config {
    pub fn new(input: &Path, engine: Engine) -> Result<Self, String> {
        Self::with_inputs(input, engine, &[])
    }
    pub fn with_inputs(
        input: &Path,
        engine: Engine,
        additional: &[PathBuf],
    ) -> Result<Self, String> {
        if additional.len() > 7 {
            return Err("At most eight TeX input folders can be enabled".into());
        }
        let input = input.canonicalize().map_err(|e| e.to_string())?;
        let root = input
            .parent()
            .ok_or("TeX document needs an input folder")?
            .to_owned();
        if input
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("tex"))
        {
            return Err("Choose a .tex root document".into());
        }
        let mut input_folders = vec![root.clone()];
        for folder in additional {
            let folder = folder.canonicalize().map_err(|e| e.to_string())?;
            if !folder.is_dir() {
                return Err("An additional TeX input must be a folder".into());
            }
            if input_folders
                .iter()
                .any(|parent| folder.starts_with(parent))
            {
                continue;
            }
            input_folders.retain(|child| !child.starts_with(&folder));
            input_folders.push(folder);
        }
        let mut base = root.clone();
        while !input_folders.iter().all(|folder| folder.starts_with(&base)) {
            if !base.pop() {
                return Err("Input folders must share a filesystem root".into());
            }
        }
        Ok(Self {
            root,
            input,
            engine,
            input_folders,
            base,
            automatic: None,
            extra_inputs: additional.to_vec(),
        })
    }
}

#[derive(Clone)]
pub struct State {
    pub status: String,
    pub error: Option<String>,
    pub output: Option<Arc<Compiled>>,
    pub revision: u64,
    pub started: Option<Instant>,
}
struct Work {
    revision: u64,
    changed: Instant,
    paused: bool,
    stopped: bool,
    busy: bool,
    cancelled: bool,
    cancel: Cancel,
    state: State,
}
struct Shared {
    work: Mutex<Work>,
    wake: mpsc::SyncSender<()>,
    updates: async_channel::Sender<()>,
}
impl Shared {
    fn changed(&self) {
        let mut work = self.work.lock().unwrap();
        work.revision += 1;
        work.cancelled = false;
        work.changed = Instant::now();
        work.cancel.store(true, Ordering::Relaxed);
        if !work.paused {
            work.state.status = "Waiting for saves…".into();
            self.notify();
        }
        let _ = self.wake.try_send(());
    }
    fn notify(&self) {
        let _ = self.updates.try_send(());
    }
}

/// One coalescing scheduler per open document; splits share its result. The
/// process-wide compiler gate admits one build, including private snapshotting.
pub struct Session {
    shared: Arc<Shared>,
}
impl Session {
    pub fn start(config: Config) -> Result<(Self, async_channel::Receiver<()>), String> {
        if !supported() {
            return Err("Local TeX preview is currently available on macOS".into());
        }
        let (wake, receive) = mpsc::sync_channel(1);
        let (updates, events) = async_channel::bounded(1);
        let shared = Arc::new(Shared {
            work: Mutex::new(Work {
                revision: 1,
                changed: Instant::now(),
                paused: false,
                stopped: false,
                busy: false,
                cancelled: false,
                cancel: Arc::new(AtomicBool::new(false)),
                state: State {
                    status: "Waiting for saves…".into(),
                    error: None,
                    output: None,
                    revision: 0,
                    started: None,
                },
            }),
            wake,
            updates,
        });
        let changed = shared.clone();
        let roots = Arc::new(Mutex::new(config.input_folders.clone()));
        let observed_roots = roots.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event)
                    if !matches!(event.kind, notify::EventKind::Access(_))
                        && (event.need_rescan()
                            || event.paths.iter().any(|path| {
                                observed_roots.lock().unwrap().iter().any(|root| {
                                    path.strip_prefix(root)
                                        .is_ok_and(|p| !snapshot::excluded(p))
                                })
                            })) =>
                {
                    changed.changed()
                }
                Err(error) => {
                    let mut work = changed.work.lock().unwrap();
                    work.cancel.store(true, Ordering::Relaxed);
                    work.paused = true;
                    work.state.started = None;
                    work.state.status = "Automatic rebuild paused".into();
                    work.state.error = Some(format!(
                        "File watcher failed: {error}. Retry Preview to resume."
                    ));
                    changed.notify();
                }
                _ => {}
            })
            .map_err(|e| e.to_string())?;
        for root in &config.input_folders {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(|e| e.to_string())?;
        }
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("tex-preview".into())
            .spawn(move || {
                let mut watcher = watcher;
                let mut built = 0;
                let mut delay = Duration::from_millis(500);
                loop {
                    let (revision, wait) = {
                        let work = worker.work.lock().unwrap();
                        if work.stopped {
                            break;
                        }
                        if work.paused || work.cancelled || built == work.revision {
                            (None, None)
                        } else {
                            (
                                Some(work.revision),
                                delay.checked_sub(work.changed.elapsed()),
                            )
                        }
                    };
                    let Some(revision) = revision else {
                        if receive.recv().is_err() {
                            break;
                        }
                        continue;
                    };
                    if let Some(wait) = wait {
                        let _ = receive.recv_timeout(wait);
                        continue;
                    }
                    let cancel = {
                        let mut work = worker.work.lock().unwrap();
                        if work.paused
                            || work.cancelled
                            || work.stopped
                            || work.revision != revision
                        {
                            continue;
                        }
                        work.cancel = Arc::new(AtomicBool::new(false));
                        work.busy = true;
                        work.state.started = Some(Instant::now());
                        work.state.status = "Building PDF…".into();
                        work.state.error = None;
                        worker.notify();
                        work.cancel.clone()
                    };
                    let start = Instant::now();
                    let result = (|| {
                        let refreshed = config.refresh_inputs()?;
                        {
                            let previous = roots.lock().unwrap().clone();
                            for root in &refreshed.input_folders {
                                if !previous.contains(root) {
                                    watcher
                                        .watch(root, RecursiveMode::Recursive)
                                        .map_err(|e| e.to_string())?;
                                }
                            }
                            for root in &previous {
                                if !refreshed.input_folders.contains(root) {
                                    let _ = watcher.unwatch(root);
                                }
                            }
                            *roots.lock().unwrap() = refreshed.input_folders.clone();
                        }
                        compile(&refreshed, &cancel)
                    })();
                    delay = (start.elapsed() / 4)
                        .clamp(Duration::from_millis(500), Duration::from_secs(2));
                    let mut work = worker.work.lock().unwrap();
                    work.busy = false;
                    work.state.started = None;
                    if work.revision != revision || work.paused || work.cancelled || work.stopped {
                        continue;
                    }
                    built = revision;
                    match result {
                        Ok(output) => {
                            work.state.output = Some(Arc::new(output));
                            work.state.revision = revision;
                            work.state.status = "Up to date".into();
                            work.state.error = None;
                        }
                        Err(error) if error.contains("changed") => {
                            work.revision += 1;
                            work.changed = Instant::now();
                            work.state.status = "Waiting for saves…".into();
                        }
                        Err(error) => {
                            work.state.status = if work.state.output.is_some() {
                                "Build failed · previous preview retained"
                            } else {
                                "Build failed"
                            }
                            .into();
                            work.state.error = Some(error);
                        }
                    }
                    worker.notify();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((Self { shared }, events))
    }
    pub fn state(&self) -> State {
        self.shared.work.lock().unwrap().state.clone()
    }
    pub fn refresh(&self) {
        self.shared.changed();
    }
    /// Cancel this revision; the next save or explicit retry resumes normally.
    pub fn cancel_current(&self) {
        let mut work = self.shared.work.lock().unwrap();
        work.cancelled = true;
        work.cancel.store(true, Ordering::Relaxed);
        work.state.started = None;
        work.state.status = "Preview cancelled".into();
        self.shared.notify();
        let _ = self.shared.wake.try_send(());
    }
    pub fn set_paused(&self, paused: bool) {
        let mut work = self.shared.work.lock().unwrap();
        if work.paused == paused {
            return;
        }
        work.paused = paused;
        if paused {
            work.state.started = None;
        }
        work.cancel.store(true, Ordering::Relaxed);
        if paused && work.busy {
            work.revision += 1;
        }
        work.changed = Instant::now();
        work.state.status = if paused {
            "Paused"
        } else if work.cancelled {
            "Preview cancelled"
        } else if work.state.error.is_some() {
            if work.state.output.is_some() {
                "Build failed · previous preview retained"
            } else {
                "Build failed"
            }
        } else if work.state.revision == work.revision {
            "Up to date"
        } else {
            "Waiting for saves…"
        }
        .into();
        let _ = self.shared.wake.try_send(());
        self.shared.notify();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let mut work = self.shared.work.lock().unwrap();
        work.stopped = true;
        work.cancel.store(true, Ordering::Relaxed);
        let _ = self.shared.wake.try_send(());
    }
}
