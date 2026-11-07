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
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    expect(window, &other_path, Some(Decoration::Staged), cx);
    window
        .update(cx, |workspace, _, _| {
            assert_eq!(workspace.git.snapshot.as_ref().unwrap().root, other_root);
            assert!(
                workspace
                    .git
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .status(&path)
                    .is_none()
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.run_until_parked();
    window
        .update(cx, |workspace, window, _| {
            assert!(workspace.git.scope.is_none());
            assert!(workspace.git.snapshot.is_none());
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
