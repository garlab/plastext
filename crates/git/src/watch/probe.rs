//! Opt-in diagnostic of real repositories, never part of the ordinary test run.
use super::*;
use std::{
    fs,
    time::{Duration, Instant},
};

#[test]
#[ignore = "set PLASTEXT_GIT_WATCH_ROOT and PLASTEXT_GIT_TRACE=1 to capture native events"]
fn watch_directory_events() {
    assert!(crate::trace::enabled(), "set PLASTEXT_GIT_TRACE=1");
    let directory = fs::canonicalize(
        std::env::var_os("PLASTEXT_GIT_WATCH_ROOT")
            .expect("set PLASTEXT_GIT_WATCH_ROOT to the folder to observe"),
    )
    .unwrap();
    let seconds = std::env::var("PLASTEXT_GIT_WATCH_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(180)
        .clamp(1, 3600);
    let mut candidates = vec![directory.clone()];
    candidates.extend(
        fs::read_dir(&directory)
            .unwrap()
            .take(4096)
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path()),
    );
    candidates.sort();
    let mut plans = BTreeMap::new();
    for path in candidates {
        let Ok(repo) = gix::discover(&path) else {
            continue;
        };
        let Some(root) = repo.workdir() else {
            continue;
        };
        let root = fs::canonicalize(root).unwrap();
        if plans.contains_key(&root) || plans.len() >= 64 {
            continue;
        }
        let index = repo.index_or_empty().unwrap();
        let metadata = [
            fs::canonicalize(repo.git_dir()).unwrap(),
            fs::canonicalize(repo.common_dir()).unwrap(),
        ];
        let plan = Plan::from_index(&index, &root, &metadata, &AtomicBool::new(false)).unwrap();
        crate::git_trace!(
            "probe_repository root={root:?} directory_filter_complete={}",
            plan.directories.is_some()
        );
        plans.insert(root, Arc::new(plan));
    }
    crate::git_trace!(
        "probe_start root={directory:?} repositories={} seconds={seconds}",
        plans.len()
    );
    let (watches, events) = Watches::new();
    watches.set(plans.into_values().collect());
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        if events.try_recv().is_ok() {
            let changes = watches.take();
            crate::git_trace!(
                "probe_batch dirty={:?} healthy={:?}",
                changes.dirty,
                changes.healthy
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    crate::git_trace!("probe_end elapsed_ms={}", started.elapsed().as_millis());
    // Allow the asynchronous diagnostic writer to consume its final record.
    std::thread::sleep(Duration::from_millis(100));
}
