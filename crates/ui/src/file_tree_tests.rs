use super::*;
use gpui::TestAppContext;
use std::{fs, path::Path};

fn open(cx: &mut TestAppContext, root: &Path) -> Entity<FileTreeView> {
    cx.update(crate::init);
    let tree = cx.update(|cx| {
        let icons = IconThemes::shared(&Default::default(), cx);
        cx.new(|cx| FileTreeView::new(root.to_owned(), TreeOptions::default(), icons, cx))
    });
    cx.run_until_parked();
    tree
}

fn click(cx: &mut TestAppContext, tree: &Entity<FileTreeView>, path: &Path) {
    tree.update(cx, |tree, cx| {
        let index = tree
            .tree
            .rows()
            .iter()
            .position(|row| row.entry.path == path)
            .unwrap();
        tree.click(index, 1, cx);
    });
    cx.run_until_parked();
}

fn paths(cx: &mut TestAppContext, tree: &Entity<FileTreeView>) -> Vec<PathBuf> {
    tree.read_with(cx, |tree, _| {
        tree.tree
            .rows()
            .iter()
            .map(|r| r.entry.path.clone())
            .collect()
    })
}

fn rescan(cx: &mut TestAppContext, tree: &Entity<FileTreeView>) {
    tree.update(cx, |tree, cx| {
        tree.refresh_external(
            &Batch {
                rescan: true,
                ..Batch::default()
            },
            false,
            cx,
        )
    });
    cx.run_until_parked();
}

#[gpui::test]
fn refresh_updates_nested_entries_and_preserves_other_expansions_and_selection(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for folder in ["a", "b"] {
        fs::create_dir(root.join(folder)).unwrap();
        fs::write(root.join(folder).join("old.txt"), "").unwrap();
    }
    let tree = open(cx, root);
    click(cx, &tree, &root.join("a"));
    click(cx, &tree, &root.join("b"));
    tree.update(cx, |tree, cx| tree.select(Some(root.join("b/old.txt")), cx));
    fs::rename(root.join("a/old.txt"), root.join("a/new.txt")).unwrap();
    fs::write(root.join("a/created.txt"), "").unwrap();
    rescan(cx, &tree);
    assert_eq!(
        paths(cx, &tree),
        ["a", "a/created.txt", "a/new.txt", "b", "b/old.txt"].map(|p| root.join(p))
    );
    tree.read_with(cx, |tree, _| {
        assert_eq!(tree.selected(), Some(root.join("b/old.txt")));
        assert_eq!(
            tree.watched_directories(),
            [root.to_owned(), root.join("a"), root.join("b")]
        );
    });
    fs::remove_file(root.join("b/old.txt")).unwrap();
    rescan(cx, &tree);
    tree.read_with(cx, |tree, _| assert!(tree.selected().is_none()));
}

#[gpui::test]
fn reopening_collapsed_folders_refreshes_cached_descendants(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a/nested")).unwrap();
    fs::write(root.join("a/nested/old.txt"), "").unwrap();
    let tree = open(cx, root);
    click(cx, &tree, &root.join("a"));
    click(cx, &tree, &root.join("a/nested"));
    click(cx, &tree, &root.join("a"));
    tree.read_with(cx, |tree, _| {
        assert_eq!(tree.watched_directories(), [root.to_owned()])
    });
    fs::rename(root.join("a/nested/old.txt"), root.join("a/nested/new.txt")).unwrap();
    click(cx, &tree, &root.join("a"));
    assert_eq!(
        paths(cx, &tree),
        ["a", "a/nested", "a/nested/new.txt"].map(|p| root.join(p))
    );
}

#[gpui::test]
fn metadata_poll_reloads_changed_directories_without_rebuilding_unchanged_rows(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("old.txt"), "").unwrap();
    let tree = open(cx, root);
    let original = tree.read_with(cx, |tree, _| tree.tree.rows()[0].entry.clone());
    tree.update(cx, |tree, cx| {
        tree.refresh_external(&Batch::default(), true, cx)
    });
    cx.run_until_parked();
    tree.read_with(cx, |tree, _| {
        assert!(Arc::ptr_eq(&original, &tree.tree.rows()[0].entry))
    });
    fs::rename(root.join("old.txt"), root.join("new.txt")).unwrap();
    // Force a distinct directory timestamp even on coarse-resolution filesystems.
    let changed = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    fs::File::open(root)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(changed))
        .unwrap();
    tree.update(cx, |tree, cx| {
        tree.refresh_external(&Batch::default(), true, cx)
    });
    cx.run_until_parked();
    assert_eq!(paths(cx, &tree), [root.join("new.txt")]);
}

#[gpui::test]
fn changes_during_a_listing_coalesce_and_keep_the_latest_names(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("old.txt"), "").unwrap();
    let tree = open(cx, root);
    tree.update(cx, |tree, cx| {
        tree.list(root.to_owned(), cx);
        fs::rename(root.join("old.txt"), root.join("new.txt")).unwrap();
        for _ in 0..10 {
            tree.list(root.to_owned(), cx);
        }
        assert_eq!(tree.listings.len(), 1);
        assert_eq!(tree.relist.len(), 1);
    });
    cx.run_until_parked();
    assert_eq!(paths(cx, &tree), [root.join("new.txt")]);
    tree.read_with(cx, |tree, _| {
        assert!(tree.listings.is_empty());
        assert!(tree.relist.is_empty());
    });
}
