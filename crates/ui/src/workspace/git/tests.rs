use super::*;
use gpui::{TestAppContext, WindowHandle};

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn refresh(window: WindowHandle<Workspace>, cx: &mut TestAppContext) {
    window
        .update(cx, |workspace, _, _| {
            for _ in 0..100 {
                workspace.git.request();
            }
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
}

fn expect(
    window: WindowHandle<Workspace>,
    path: &Path,
    expected: Option<Decoration>,
    cx: &mut TestAppContext,
) {
    window
        .update(cx, |workspace, window, cx| {
            let snapshot = workspace.git.snapshot.as_ref().unwrap();
            assert_eq!(
                snapshot.status(path).and_then(FileStatus::decoration),
                expected
            );
            if let Some(tree) = &workspace.tree {
                assert_eq!(tree.read(cx).git_status(path), snapshot.status(path));
            }
            let theme = Theme::get(window, cx);
            assert_eq!(
                color(snapshot.status(path), theme).is_some(),
                expected.is_some()
            );
            assert!(!workspace.git.failed);
        })
        .unwrap();
}

#[gpui::test]
fn tree_and_tabs_share_status_and_recover_after_failed_refresh(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let path = root.join("file.txt");
    git(&root, &["init", "-qb", "main"]);
    std::fs::write(&path, "original\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    std::fs::write(&path, "modified\n").unwrap();
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace = Workspace::new(Some(root.clone()), Arc::default(), window, cx);
        workspace.open_file(path.clone(), None, window, cx);
        workspace
    });
    cx.run_until_parked();
    expect(window, &path, Some(Decoration::Modified), cx);
    git(&root, &["add", "."]);
    refresh(window, cx);
    expect(window, &path, Some(Decoration::Staged), cx);
    std::fs::write(&path, "modified after staging\n").unwrap();
    refresh(window, cx);
    expect(window, &path, Some(Decoration::Mixed), cx);
    let index = std::fs::read(root.join(".git/index")).unwrap();
    std::fs::write(root.join(".git/index"), "invalid index").unwrap();
    refresh(window, cx);
    window
        .update(cx, |workspace, _, _| {
            assert!(workspace.git.failed);
            assert_eq!(
                workspace
                    .git
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .status(&path)
                    .unwrap()
                    .decoration(),
                Some(Decoration::Mixed)
            );
        })
        .unwrap();
    std::fs::write(root.join(".git/index"), index).unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "saved"]);
    refresh(window, cx);
    expect(window, &path, None, cx);
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn folderless_window_discovers_the_open_files_repository(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-qb", "main"]);
    let path = root.join("file.txt");
    std::fs::write(&path, "staged\n").unwrap();
    git(&root, &["add", "."]);
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace = Workspace::new(None, Arc::default(), window, cx);
        workspace.open_file(path.clone(), None, window, cx);
        workspace
    });
    cx.run_until_parked();
    expect(window, &path, Some(Decoration::Staged), cx);
    let other = tempfile::tempdir().unwrap();
    let other_root = std::fs::canonicalize(other.path()).unwrap();
    let other_path = other_root.join("other.txt");
    git(&other_root, &["init", "-qb", "main"]);
    std::fs::write(&other_path, "also staged\n").unwrap();
    git(&other_root, &["add", "."]);
    window
        .update(cx, |workspace, window, cx| {
            workspace.open_file(other_path.clone(), None, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    expect(window, &other_path, Some(Decoration::Staged), cx);
    window
        .update(cx, |workspace, _, _| {
            let snapshot = workspace.git.snapshot.as_ref().unwrap();
            assert_eq!(snapshot.roots().count(), 2);
            assert!(snapshot.repository(&other_root).is_some());
            assert_eq!(
                snapshot.status(&path).unwrap().decoration(),
                Some(Decoration::Staged)
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, _| {
            assert!(workspace.git.scopes.is_empty());
            assert_eq!(workspace.git.snapshot.as_ref().unwrap().roots().count(), 2);
            window.remove_window();
        })
        .unwrap();
}

fn toggle_folder(window: WindowHandle<Workspace>, path: &Path, cx: &mut TestAppContext) {
    window
        .update(cx, |workspace, window, cx| {
            let tree = workspace.tree.as_ref().unwrap();
            tree.focus_handle(cx).focus(window);
            tree.update(cx, |tree, cx| tree.select(Some(path.to_owned()), cx));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    refresh(window, cx);
}

#[gpui::test]
fn parent_folder_discovers_expanded_repositories_and_keeps_failures_separate(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let repos = [root.join("alpha"), root.join("beta")];
    for repo in &repos {
        std::fs::create_dir(repo).unwrap();
        git(repo, &["init", "-qb", "main"]);
        std::fs::create_dir(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/file.txt"), "initial\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "initial"]);
        std::fs::write(repo.join("src/file.txt"), "modified\n").unwrap();
    }
    let paths = repos.each_ref().map(|repo| repo.join("src/file.txt"));
    cx.update(crate::init);
    let window =
        cx.add_window(|window, cx| Workspace::new(Some(root.clone()), Arc::default(), window, cx));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, _| {
            // Listed child repositories are discovered before being expanded.
            assert_eq!(workspace.git.snapshot.as_ref().unwrap().roots().count(), 2);
        })
        .unwrap();
    toggle_folder(window, &repos[0], cx);
    expect(window, &paths[0], Some(Decoration::Modified), cx);
    toggle_folder(window, &repos[1], cx);
    toggle_folder(window, &repos[0].join("src"), cx);
    expect(window, &paths[0], Some(Decoration::Modified), cx);
    expect(window, &paths[1], Some(Decoration::Modified), cx);
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.git.snapshot.as_ref().unwrap().roots().count(), 2);
        })
        .unwrap();

    // A broken first repository must not prevent the second one refreshing.
    let index = std::fs::read(repos[0].join(".git/index")).unwrap();
    std::fs::write(repos[0].join(".git/index"), "invalid").unwrap();
    git(&repos[1], &["add", "."]);
    refresh(window, cx);
    window
        .update(cx, |workspace, _, cx| {
            assert!(workspace.git.failed);
            let snapshot = workspace.git.snapshot.as_ref().unwrap();
            assert_eq!(
                snapshot.status(&paths[0]).unwrap().decoration(),
                Some(Decoration::Modified)
            );
            assert_eq!(
                snapshot.status(&paths[1]).unwrap().decoration(),
                Some(Decoration::Staged)
            );
            assert_eq!(
                workspace
                    .tree
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .git_status(&paths[1]),
                snapshot.status(&paths[1])
            );
        })
        .unwrap();
    std::fs::write(repos[0].join(".git/index"), index).unwrap();
    window
        .update(cx, |workspace, window, cx| {
            workspace.open_file(paths[0].clone(), None, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    toggle_folder(window, &repos[0], cx);
    expect(window, &paths[0], Some(Decoration::Modified), cx);
    window
        .update(cx, |workspace, window, cx| {
            workspace.panes[&workspace.active_pane]
                .active_tab()
                .unwrap()
                .view
                .focus_handle(cx)
                .focus(window);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.run_until_parked();
    refresh(window, cx);
    window
        .update(cx, |workspace, window, _| {
            let snapshot = workspace.git.snapshot.as_ref().unwrap();
            assert_eq!(snapshot.roots().count(), 2);
            assert!(snapshot.status(&paths[0]).is_some());
            window.remove_window();
        })
        .unwrap();
}

#[test]
fn slow_scans_reduce_refresh_frequency() {
    assert_eq!(cooldown(Duration::from_millis(20)), Duration::from_secs(1));
    assert_eq!(cooldown(Duration::from_secs(2)), Duration::from_secs(8));
    assert_eq!(cooldown(Duration::from_secs(90)), Duration::from_secs(60));
}

#[gpui::test]
fn watched_repository_is_idle_until_an_event_and_defers_work_while_inactive(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-qb", "main"]);
    let path = root.join("file.txt");
    std::fs::write(&path, "initial\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "initial"]);
    cx.update(crate::init);
    let window =
        cx.add_window(|window, cx| Workspace::new(Some(root.clone()), Arc::default(), window, cx));
    cx.run_until_parked();
    let count = window
        .update(cx, |workspace, _, _| {
            workspace.git.observe_watches(plastext_git::watch::Changes {
                healthy: Some([root.clone()].into()),
                ..Default::default()
            });
            workspace.git.scan_counts[&root]
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, _| {
            workspace.git.changed_file(&root.join("generated.part"));
            assert!(workspace.git.pending.paths.is_empty());
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(180));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.git.scan_counts[&root], count);
            workspace.git.set_active(false);
        })
        .unwrap();
    cx.run_until_parked();
    std::fs::write(&path, "changed while inactive\n").unwrap();
    window
        .update(cx, |workspace, _, _| {
            workspace.git.observe_watches(plastext_git::watch::Changes {
                dirty: [root.clone()].into(),
                ..Default::default()
            })
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(40));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.git.scan_counts[&root], count);
            workspace.git.set_active(true);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    expect(window, &path, Some(Decoration::Modified), cx);
    window
        .update(cx, |workspace, window, _| {
            assert_eq!(workspace.git.scan_counts[&root], count + 1);
            window.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn refresh_indicator_delays_batches_and_cancels_finished_timers(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let window = cx.add_window(|window, cx| Workspace::new(None, Arc::default(), window, cx));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, cx| {
            workspace.git.start_refresh(cx);
            assert_eq!(workspace.git.label(), None);
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, cx| {
            assert_eq!(workspace.git.label(), None);
            workspace.git.finish_refresh(false, cx);
            workspace.git.start_refresh(cx);
        })
        .unwrap();
    cx.run_until_parked();
    // The previous batch's timer must not reveal this newer, short batch.
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, cx| {
            assert_eq!(workspace.git.label(), None);
            workspace.git.start_refresh(cx); // another repository in the same batch
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(150));
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, cx| {
            assert_eq!(workspace.git.label(), Some("Git: refreshing…"));
            workspace.git.finish_refresh(false, cx);
            assert_eq!(workspace.git.label(), None);
            workspace.git.start_refresh(cx);
            workspace.git.finish_refresh(true, cx);
            assert_eq!(workspace.git.label(), Some("Git: some status unavailable"));
            window.remove_window();
        })
        .unwrap();
}

#[gpui::test]
fn collapse_reopen_keeps_colors_without_scanning_and_disk_events_are_targeted(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let repos = [root.join("alpha"), root.join("beta")];
    for repo in &repos {
        std::fs::create_dir(repo).unwrap();
        git(repo, &["init", "-qb", "main"]);
        std::fs::create_dir(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/file.txt"), "initial\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "initial"]);
        std::fs::write(repo.join("src/file.txt"), "modified\n").unwrap();
    }
    cx.update(crate::init);
    let window =
        cx.add_window(|window, cx| Workspace::new(Some(root.clone()), Arc::default(), window, cx));
    cx.run_until_parked();
    let (counts, cached) = window
        .update(cx, |workspace, _, cx| {
            let tree = workspace.tree.as_ref().unwrap().read(cx);
            for repo in &repos {
                assert_eq!(
                    tree.git_folder_status(repo).unwrap().decoration(),
                    Some(Decoration::Modified)
                );
                assert_eq!(
                    tree.git_folder_status(&repo.join("src"))
                        .unwrap()
                        .decoration(),
                    Some(Decoration::Modified)
                );
            }
            (
                workspace.git.scan_counts.clone(),
                workspace
                    .git
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .repository(&repos[0])
                    .unwrap()
                    .clone(),
            )
        })
        .unwrap();
    assert_eq!(counts[&repos[0]], 1);
    assert_eq!(counts[&repos[1]], 1);
    for _ in 0..4 {
        window
            .update(cx, |workspace, window, cx| {
                let tree = workspace.tree.as_ref().unwrap();
                tree.focus_handle(cx).focus(window);
                tree.update(cx, |tree, cx| tree.select(Some(repos[0].clone()), cx));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "enter");
        // Assert before any timer: reopening has colors immediately.
        window
            .update(cx, |workspace, _, cx| {
                let tree = workspace.tree.as_ref().unwrap().read(cx);
                assert_eq!(
                    tree.git_status(&repos[0].join("src/file.txt"))
                        .unwrap()
                        .decoration(),
                    Some(Decoration::Modified)
                );
                assert_eq!(
                    tree.git_folder_status(&repos[0]).unwrap().decoration(),
                    Some(Decoration::Modified)
                );
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
    }
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.git.scan_counts, counts);
            assert!(Arc::ptr_eq(
                &cached,
                workspace
                    .git
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .repository(&repos[0])
                    .unwrap()
            ));
            workspace.git.pending.paths.insert(root.join("unrelated"));
            workspace.git.wake();
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.git.scan_counts, counts)
        })
        .unwrap();

    git(&repos[0], &["add", "."]);
    git(&repos[0], &["commit", "-qm", "saved"]);
    window
        .update(cx, |workspace, _, _| {
            workspace
                .git
                .pending
                .paths
                .insert(repos[0].join(".git/index"));
            workspace.git.wake();
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, cx| {
            assert_eq!(workspace.git.scan_counts[&repos[1]], counts[&repos[1]]);
            assert!(workspace.git.scan_counts[&repos[0]] > counts[&repos[0]]);
            let tree = workspace.tree.as_ref().unwrap().read(cx);
            assert!(tree.git_folder_status(&repos[0]).is_none());
            assert!(tree.git_folder_status(&repos[1]).is_some());
            window.remove_window();
        })
        .unwrap();
}
