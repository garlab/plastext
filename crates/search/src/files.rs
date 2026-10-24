use std::{collections::HashMap, sync::Arc};

use nucleo::{
    Config, Injector, Nucleo, Status,
    pattern::{CaseMatching, Normalization},
};

struct RecentFile {
    path: String,
    rank: usize,
}

/// Streaming Quick Open matching. Matching recent files form a boosted
/// group in MRU order; other files retain nucleo's relevance order. Keeping
/// the groups separate avoids sorting or copying a million results on the UI
/// thread. Only paths found by the project's ignore-aware walk are eligible.
pub struct FileMatcher {
    files: Nucleo<String>,
    recent: Nucleo<RecentFile>,
    ranks: Arc<HashMap<String, usize>>,
    recent_matches: Vec<String>,
}

#[derive(Clone)]
pub struct FileInjector {
    files: Injector<String>,
    recent: Injector<RecentFile>,
    ranks: Arc<HashMap<String, usize>>,
}

impl FileMatcher {
    pub fn new(recent: &[String]) -> Self {
        let mut ranks = HashMap::new();
        // Match the workspace's bounded history, even for other callers.
        for (rank, path) in recent.iter().take(200).enumerate() {
            ranks.entry(path.clone()).or_insert(rank);
        }
        Self {
            files: Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), None, 1),
            recent: Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), Some(1), 1),
            ranks: Arc::new(ranks),
            recent_matches: Vec::new(),
        }
    }

    pub fn injector(&self) -> FileInjector {
        FileInjector {
            files: self.files.injector(),
            recent: self.recent.injector(),
            ranks: self.ranks.clone(),
        }
    }

    pub fn reparse(&mut self, pattern: &str, append: bool) {
        for matcher in [&mut self.files.pattern, &mut self.recent.pattern] {
            matcher.reparse(
                0,
                pattern,
                CaseMatching::Smart,
                Normalization::Smart,
                append,
            );
        }
    }

    /// Nonblocking: background workers score paths; only the small recent
    /// group is ordered here. Call again while `running` is true.
    pub fn tick(&mut self) -> Status {
        let files = self.files.tick(0);
        let recent = self.recent.tick(0);
        if recent.changed {
            let mut matches: Vec<_> = self
                .recent
                .snapshot()
                .matched_items(..)
                .map(|item| item.data)
                .collect();
            matches.sort_unstable_by_key(|item| item.rank);
            self.recent_matches = matches.into_iter().map(|item| item.path.clone()).collect();
        }
        Status {
            changed: files.changed || recent.changed,
            running: files.running || recent.running,
        }
    }

    pub fn item_count(&self) -> usize {
        self.files.snapshot().item_count() as usize + self.recent.snapshot().item_count() as usize
    }

    pub fn matched_count(&self) -> usize {
        self.recent_matches.len() + self.files.snapshot().matched_item_count() as usize
    }

    pub fn item(&self, index: usize) -> Option<&str> {
        if index < self.recent_matches.len() {
            return self.recent_matches.get(index).map(String::as_str);
        }
        self.files
            .snapshot()
            .get_matched_item((index - self.recent_matches.len()).try_into().ok()?)
            .map(|item| item.data.as_str())
    }
}

impl FileInjector {
    pub fn push(&self, path: String) {
        if let Some(&rank) = self.ranks.get(&path) {
            self.recent
                .push(RecentFile { path, rank }, |item, columns| {
                    columns[0] = item.path.as_str().into()
                });
        } else {
            self.files
                .push(path, |path, columns| columns[0] = path.as_str().into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn settle(matcher: &mut FileMatcher) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while matcher.tick().running {
            assert!(Instant::now() < deadline, "matcher didn't finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn boosts_only_matching_recent_paths_without_duplicates() {
        let mut matcher = FileMatcher::new(&[
            "old/deep/lib.rs".into(),
            "src/lib.rs".into(),
            "missing/lib.rs".into(),
            "README.md".into(),
        ]);
        let injector = matcher.injector();
        for path in ["lib.rs", "src/lib.rs", "old/deep/lib.rs", "README.md"] {
            injector.push(path.into());
        }
        matcher.reparse("lib", false);
        settle(&mut matcher);
        assert_eq!(matcher.matched_count(), 3);
        assert_eq!(
            (0..3).map(|i| matcher.item(i).unwrap()).collect::<Vec<_>>(),
            ["old/deep/lib.rs", "src/lib.rs", "lib.rs"]
        );
        assert_eq!(matcher.item_count(), 4);
        matcher.reparse("README", false);
        settle(&mut matcher);
        assert_eq!(matcher.matched_count(), 1);
        assert_eq!(matcher.item(0), Some("README.md"));
        matcher.reparse("LIB", false);
        settle(&mut matcher);
        assert_eq!(
            matcher.matched_count(),
            0,
            "smart case also applies to recent files"
        );
    }

    #[test]
    fn files_stream_in_under_the_current_query() {
        let mut matcher = FileMatcher::new(&["later.rs".into()]);
        matcher.reparse("rs", false);
        matcher.injector().push("first.rs".into());
        settle(&mut matcher);
        matcher.injector().push("later.rs".into());
        settle(&mut matcher);
        assert_eq!(matcher.item(0), Some("later.rs"));
        assert_eq!(matcher.item(1), Some("first.rs"));
        assert_eq!(matcher.item(2), None);
    }
}
