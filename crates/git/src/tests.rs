use super::*;
use std::{fs, process::Command, time::Instant};

fn git(root: &Path, args: &[&str]) {
    let output = command(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ]);
    cmd
}
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    dir
}
fn scan(root: &Path) -> Snapshot {
    status(root, Arc::new(AtomicBool::new(false)), Limits::default())
        .unwrap()
        .unwrap()
}
fn decoration(snapshot: &Snapshot, path: &Path) -> Option<Decoration> {
    snapshot.status(path).and_then(FileStatus::decoration)
}

#[test]
fn staged_modified_mixed_and_clean_follow_index_and_head_without_writes() {
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    for file in ["staged", "modified", "mixed", "clean", "删除 é.txt"] {
        fs::write(root.join(file), "before\n").unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    for file in ["staged", "mixed", "modified"] {
        fs::write(root.join(file), "after\n").unwrap();
    }
    git(&root, &["add", "staged", "mixed"]);
    fs::write(root.join("mixed"), "after staging\n").unwrap();
    fs::write(root.join("untracked"), "new").unwrap();
    fs::remove_file(root.join("删除 é.txt")).unwrap();
    git(&root, &["add", "删除 é.txt"]);
    let index = fs::read(root.join(".git/index")).unwrap();
    let config = fs::read(root.join(".git/config")).unwrap();
    let snapshot = scan(&root);
    assert_eq!(
        decoration(&snapshot, &root.join("staged")),
        Some(Decoration::Staged)
    );
    assert_eq!(
        decoration(&snapshot, &root.join("modified")),
        Some(Decoration::Modified)
    );
    assert_eq!(
        decoration(&snapshot, &root.join("mixed")),
        Some(Decoration::Mixed)
    );
    assert_eq!(
        decoration(&snapshot, &root.join("删除 é.txt")),
        Some(Decoration::Staged)
    );
    assert_eq!(decoration(&snapshot, &root.join("clean")), None);
    assert_eq!(decoration(&snapshot, &root.join("untracked")), None);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(root.join(".git/config")).unwrap(), config);
    git(&root, &["reset", "-q", "HEAD", "staged"]);
    assert_eq!(
        decoration(&scan(&root), &root.join("staged")),
        Some(Decoration::Modified)
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "save"]);
    assert_eq!(scan(&root).changed_files(), 0);
}

#[test]
fn unborn_head_renames_intent_to_add_and_limits() {
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(root.join("new"), "new\n").unwrap();
    git(&root, &["add", "new"]);
    assert_eq!(
        decoration(&scan(&root), &root.join("new")),
        Some(Decoration::Staged)
    );
    git(&root, &["commit", "-qm", "initial"]);
    git(&root, &["mv", "new", "renamed"]);
    fs::write(root.join("intent"), "intent\n").unwrap();
    git(&root, &["add", "-N", "intent"]);
    let snapshot = scan(&root);
    assert_eq!(
        decoration(&snapshot, &root.join("renamed")),
        Some(Decoration::Staged)
    );
    assert_eq!(
        decoration(&snapshot, &root.join("intent")),
        Some(Decoration::Modified)
    );
    let cancel = Arc::new(AtomicBool::new(false));
    assert!(
        status(
            &root,
            cancel.clone(),
            Limits {
                changed_paths: 0,
                path_bytes: 0
            }
        )
        .is_err()
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert!(status(&root, cancel, Limits::default()).is_err());
}

#[test]
fn linked_worktree_discovery_and_conflicts() {
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(root.join("file"), "base\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    let worktree = tempfile::tempdir().unwrap();
    let work = fs::canonicalize(worktree.path()).unwrap().join("linked");
    git(
        &root,
        &["worktree", "add", "-qb", "other", work.to_str().unwrap()],
    );
    fs::create_dir(work.join("sub")).unwrap();
    fs::write(work.join("file"), "other\n").unwrap();
    git(&work, &["add", "."]);
    let snapshot = scan(&work.join("sub"));
    assert_eq!(snapshot.root, work);
    assert!(snapshot.watch_directories.contains(&root.join(".git")));
    assert_eq!(
        decoration(&snapshot, &work.join("file")),
        Some(Decoration::Staged)
    );
    git(&work, &["commit", "-qm", "other"]);
    fs::write(root.join("file"), "main\n").unwrap();
    git(&root, &["commit", "-qam", "main"]);
    assert!(
        !command(&root)
            .args(["merge", "other"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        decoration(&scan(&root), &root.join("file")),
        Some(Decoration::Conflict)
    );
}

#[test]
fn no_repository_and_executable_filters_are_safe() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        status(dir.path(), Arc::default(), Limits::default())
            .unwrap()
            .is_none()
    );
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(root.join("file"), "base\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    git(
        &root,
        &[
            "config",
            "filter.probe.clean",
            "plastext-test-command-must-never-run",
        ],
    );
    git(&root, &["config", "filter.probe.required", "true"]);
    fs::write(root.join(".gitattributes"), "file filter=probe\n").unwrap();
    fs::write(root.join("file"), "changed\n").unwrap();
    assert_eq!(
        decoration(&scan(&root), &root.join("file")),
        Some(Decoration::Modified)
    );
}

#[test]
fn discovery_is_lazy_deduplicated_and_nested_status_masks_the_parent() {
    let dir = repo();
    let root = fs::canonicalize(dir.path()).unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("file"), "outer\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "outer"]);
    git(&nested, &["init", "-qb", "main"]);
    fs::write(nested.join("file"), "inner\n").unwrap();
    git(&nested, &["add", "."]);
    git(&nested, &["commit", "-qm", "inner"]);
    fs::create_dir(nested.join("src")).unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(
        discover(std::slice::from_ref(&root), &cancel).unwrap(),
        std::slice::from_ref(&root)
    );
    let roots = discover(&[root.clone(), nested.clone(), nested.join("src")], &cancel).unwrap();
    assert_eq!(roots, [root.clone(), nested.clone()]);
    let mut set = RepositorySet::for_roots(&roots, &RepositorySet::default());
    set.update(&root, Some(scan(&root)), Limits::default())
        .unwrap();
    assert!(
        set.repository(&root)
            .unwrap()
            .status(&nested.join("file"))
            .is_some()
    );
    assert_eq!(
        set.status(&nested.join("file")),
        None,
        "pending nested status masks its parent"
    );
    set.update(&nested, Some(scan(&nested)), Limits::default())
        .unwrap();
    assert_eq!(
        set.status(&nested.join("file")),
        None,
        "clean nested status masks its parent"
    );
    fs::write(nested.join("file"), "changed inner\n").unwrap();
    assert!(
        set.update(
            &nested,
            Some(scan(&nested)),
            Limits {
                changed_paths: 1,
                path_bytes: usize::MAX
            }
        )
        .is_err()
    );
    assert_eq!(
        set.status(&nested.join("file")),
        None,
        "aggregate limit retains the prior result"
    );
    set.update(&nested, Some(scan(&nested)), Limits::default())
        .unwrap();
    assert_eq!(
        set.status(&nested.join("file")).unwrap().decoration(),
        Some(Decoration::Modified)
    );
}

#[test]
#[ignore = "creates a disposable 100k-file repository for performance measurement"]
fn warm_status_100k_files() {
    let dir = repo();
    let root = dir.path();
    for folder in 0..100 {
        let path = root.join(format!("d{folder}"));
        fs::create_dir(&path).unwrap();
        for file in 0..1000 {
            fs::write(path.join(format!("f{file}")), "content\n").unwrap();
        }
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "100k files"]);
    fs::write(root.join("d0/f0"), "modified\n").unwrap();
    scan(root);
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        assert_eq!(scan(root).changed_files(), 1);
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    println!("100k tracked files, 1 modified, warm status ms: {times:?}");
}
