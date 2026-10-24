use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use ignore::{WalkBuilder, WalkState};

use crate::TreeOptions;

/// Finds every file in the project at `root` for Quick Open (spec NAV-1),
/// passing each path, relative to `root`, to `found` as soon as it's seen.
/// Ignored files are left out, as are hidden ones unless shown, and the
/// walk runs on several threads until it's done or `cancelled` is set.
/// Blocks: call it off the main thread.
pub fn walk_files(
    root: &Path,
    options: &TreeOptions,
    cancelled: &AtomicBool,
    found: impl Fn(&Path) + Sync,
) {
    let exclude = options.exclude.clone();
    let filter_root = root.to_owned();
    WalkBuilder::new(root)
        .hidden(!options.show_hidden)
        .filter_entry(move |entry| {
            let path = entry.path();
            !exclude.is_match(path.strip_prefix(&filter_root).unwrap_or(path))
        })
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if cancelled.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                if let Ok(entry) = entry
                    && entry
                        .file_type()
                        .is_some_and(|file_type| !file_type.is_dir())
                    && let Ok(relative) = entry.path().strip_prefix(root)
                {
                    found(relative);
                }
                WalkState::Continue
            })
        });
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::Mutex};

    use super::*;

    #[test]
    fn finds_files_that_are_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [".git", "src", "target", "src/nested"] {
            fs::create_dir(root.join(path)).unwrap();
        }
        for path in [
            ".gitignore",
            "README.md",
            "src/lib.rs",
            "src/nested/deep.rs",
            "target/out.bin",
            ".env",
            ".DS_Store",
        ] {
            fs::write(
                root.join(path),
                if path == ".gitignore" {
                    "target/\n"
                } else {
                    ""
                },
            )
            .unwrap();
        }
        let found = Mutex::new(Vec::new());
        walk_files(
            root,
            &TreeOptions::default(),
            &AtomicBool::new(false),
            |path| {
                found.lock().unwrap().push(path.to_owned());
            },
        );
        let mut found = found.into_inner().unwrap();
        found.sort();
        let expected: Vec<PathBuf> = ["README.md", "src/lib.rs", "src/nested/deep.rs"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(found, expected);
    }
}
