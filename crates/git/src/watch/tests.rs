use super::*;
use crate::tests::{git, repo, scan};
use std::{
    fs,
    time::{Duration, Instant},
};

fn plan() -> Plan {
    Plan {
        root: PathBuf::from("project"),
        metadata: vec![PathBuf::from("project/.git")],
        directories: Some(
            ["", "src", "src/deep"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        ),
        tracked: None,
    }
}

#[test]
fn tracked_directories_and_git_metadata_filter_build_and_lock_activity() {
    let plan = plan();
    for path in [
        "project/src/deep/file",
        "project/src",
        "project/.git/index",
        "project/.git/HEAD",
        "project/.git/refs/heads/new",
        "project/.git/packed-refs",
        "project/.git/info/attributes",
        "project",
    ] {
        assert!(plan.relevant(Path::new(path)), "{path}");
    }
    for path in [
        "project/target/debug/file",
        "project/node_modules/pkg/file",
        "project/.git/objects/ab/cd",
        "project/.git/logs/HEAD",
        "project/.git/index.lock",
        "project/.git/refs/heads/main.lock",
        "other/src/file",
    ] {
        assert!(!plan.relevant(Path::new(path)), "{path}");
    }
}

#[test]
fn tracked_paths_ignore_untracked_neighbors_but_keep_staging_attributes_and_renames() {
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::write(root.join("src/deep/file.txt"), "initial\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    let before = scan(&root).watch.unwrap();
    for path in [
        "src/deep/file.txt",
        "src/deep",
        "src",
        "SRC/DEEP/FILE.TXT",
        "SRC/DEEP",
        ".gitattributes",
        "src/deep/.gitattributes",
        ".git/index",
        ".git/HEAD",
        ".git/refs/heads/main",
        "src/deep/é.txt",
    ] {
        assert!(before.relevant(&root.join(path)), "{path}");
    }
    let mut state = Shared {
        plans: vec![before.clone()],
        ..Default::default()
    };
    for path in [
        "src/deep/file.txt.part",
        "src/deep/generated.json",
        "src/scratch.md",
        "untracked.txt",
    ] {
        for kind in [
            EventKind::Create(notify::event::CreateKind::File),
            EventKind::Modify(notify::event::ModifyKind::Any),
            EventKind::Modify(notify::event::ModifyKind::Name(
                notify::event::RenameMode::Any,
            )),
            EventKind::Remove(notify::event::RemoveKind::File),
        ] {
            assert!(
                !state.event(Ok(Event::new(kind).add_path(root.join(path)))),
                "{path}"
            );
        }
    }
    // Atomic-save temporary files can be ignored; the destination and the old
    // tracked path of a rename must still invalidate the repository.
    assert!(
        state.event(Ok(Event::new(EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::Both)
        ))
        .add_path(root.join("src/deep/file.txt.part"))
        .add_path(root.join("src/deep/file.txt"))))
    );
    assert!(
        state.event(Ok(Event::new(EventKind::Remove(
            notify::event::RemoveKind::Folder
        ))
        .add_path(root.join("src/deep"))))
    );

    fs::write(root.join("src/deep/new.txt"), "new\n").unwrap();
    assert_eq!(before, scan(&root).watch.unwrap());
    git(&root, &["add", "src/deep/new.txt"]);
    let after = scan(&root).watch.unwrap();
    assert_eq!(before.directories, after.directories);
    assert_ne!(before, after); // refresh the filter even if directories didn't change
    assert!(after.relevant(&root.join("src/deep/new.txt")));
    assert!(!after.relevant(&root.join("src/deep/new.txt.part")));
    git(&root, &["rm", "--cached", "src/deep/new.txt"]);
    assert!(
        !scan(&root)
            .watch
            .unwrap()
            .relevant(&root.join("src/deep/new.txt"))
    );
}

#[test]
#[cfg(target_os = "macos")]
fn native_watch_filters_untracked_neighbors_and_detects_their_staging() {
    // This checks macOS's file-level delivery. Other backends may also emit
    // coarse parent-directory notifications, which we deliberately retain.
    let container = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(container.path())
        .unwrap()
        .join("repository");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/tracked.txt"), "initial\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    let (watches, events) = Watches::new();
    watches.set(vec![scan(&root).watch.unwrap()]);
    wait(&watches, &events, |changes| {
        changes
            .healthy
            .as_ref()
            .is_some_and(|set| set.contains(&root))
    });
    // Drain delayed native events from fixture creation before testing silence.
    std::thread::sleep(Duration::from_secs(1));
    while events.try_recv().is_ok() {}
    watches.take();
    let scratch = root.join("src/generated.part");
    for i in 0..20 {
        fs::write(&scratch, format!("generated {i}\n")).unwrap();
    }
    let generated = root.join("src/generated.txt");
    fs::rename(scratch, &generated).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        watches.take().dirty.is_empty(),
        "untracked writes triggered status"
    );
    git(&root, &["add", "src/generated.txt"]);
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
    watches.set(vec![scan(&root).watch.unwrap()]);
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
    fs::write(generated, "now tracked\n").unwrap();
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
}

#[test]
fn event_storms_are_coalesced_and_overflow_requires_reconciliation() {
    let mut state = Shared {
        plans: vec![Arc::new(plan())],
        ..Default::default()
    };
    for _ in 0..10_000 {
        assert!(
            !state.event(Ok(Event::new(EventKind::Modify(
                notify::event::ModifyKind::Any
            ))
            .add_path(PathBuf::from("project/target/debug/dependency/file"))))
        );
    }
    assert!(state.changes.dirty.is_empty());
    for _ in 0..10_000 {
        state.event(Ok(Event::new(EventKind::Modify(
            notify::event::ModifyKind::Any,
        ))
        .add_path(PathBuf::from("project/src/deep/file"))));
    }
    assert_eq!(state.changes.dirty.len(), 1);
    state.healthy.insert(PathBuf::from("project"));
    let event = Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
    assert!(state.event(Ok(event)));
    assert_eq!(state.changes.healthy, Some(HashSet::new()));
    assert_eq!(state.changes.dirty.len(), 1);
}

#[test]
fn linked_worktree_private_index_is_distinct_from_common_metadata() {
    let mut linked = plan();
    linked.root = PathBuf::from("linked");
    linked
        .metadata
        .insert(0, PathBuf::from("project/.git/worktrees/linked"));
    assert!(linked.relevant(Path::new("project/.git/worktrees/linked/index")));
    assert!(linked.relevant(Path::new("project/.git/worktrees/linked/HEAD")));
    assert!(linked.relevant(Path::new("project/.git/refs/heads/linked")));
    assert!(!plan().relevant(Path::new("project/.git/worktrees/linked/index")));
}

#[test]
fn unavailable_watch_keeps_polling_fallback_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap().join("missing");
    let plan = Plan {
        root: root.clone(),
        metadata: Vec::new(),
        directories: Some([PathBuf::new()].into()),
        tracked: None,
    };
    let (watches, events) = Watches::new();
    watches.set(vec![Arc::new(plan)]);
    wait(&watches, &events, |changes| {
        changes
            .healthy
            .as_ref()
            .is_some_and(|healthy| !healthy.contains(&root))
    });
}

#[track_caller]
fn wait(
    watches: &Watches,
    events: &async_channel::Receiver<()>,
    predicate: impl Fn(&Changes) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if events.try_recv().is_ok() && predicate(&watches.take()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Git watcher did not deliver expected change"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_watch_observes_collapsed_descendants_replacement_and_staging() {
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("src/deep")).unwrap();
    let path = root.join("src/deep/file");
    fs::write(&path, "initial\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    let plan = scan(&root).watch.unwrap();
    let (watches, events) = Watches::new();
    watches.set(vec![plan.clone()]);
    wait(&watches, &events, |changes| {
        changes
            .healthy
            .as_ref()
            .is_some_and(|healthy| healthy.contains(&root))
    });
    fs::write(&path, "changed\n").unwrap();
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
    let replacement = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
    fs::write(replacement.path(), "replacement\n").unwrap();
    replacement.persist(&path).unwrap();
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
    git(&root, &["add", "."]);
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
    fs::remove_file(&path).unwrap();
    wait(&watches, &events, |changes| changes.dirty.contains(&root));
    // A tracked file is never ignored just because of its directory's name.
    fs::create_dir_all(root.join("target/tracked")).unwrap();
    fs::write(root.join("target/tracked/file"), "tracked\n").unwrap();
    git(&root, &["add", "target/tracked/file"]);
    let plan = scan(&root).watch.unwrap();
    assert!(plan.relevant(&root.join("target/tracked/file")));
    assert!(!plan.relevant(&root.join("target/debug/dependency/file")));
}
