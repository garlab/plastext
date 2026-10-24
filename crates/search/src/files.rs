use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

use nucleo::{
    Config, Matcher, Utf32String,
    pattern::{CaseMatching, Normalization, Pattern},
};
use rayon::prelude::*;

/// Quick Open lists the best matches; refining the query can reach every
/// indexed path. Count all matches without sorting a million unseen rows.
pub const MAX_FILE_RESULTS: usize = 256;
const CHUNK_SIZE: usize = 2048;
const CACHED_QUERIES: usize = 32;

struct Candidate {
    path: Arc<str>,
    text: Utf32String,
    len: usize,
    id: usize,
    recent: usize,
    ascii_mask: u64,
}

#[derive(Default)]
struct Index {
    chunks: Vec<Arc<[Candidate]>>,
    pending: Vec<Candidate>,
    count: usize,
}

impl Index {
    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.chunks.push(std::mem::take(&mut self.pending).into());
        }
    }
}

struct Shared {
    index: Mutex<Index>,
    count: AtomicUsize,
    ranks: HashMap<String, usize>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Snapshot {
    paths: Vec<Arc<str>>,
    matched: usize,
    count: usize,
}

struct Completed {
    generation: usize,
    snapshot: Arc<Snapshot>,
    complete: bool,
}

pub struct FileMatchStatus {
    pub changed: bool,
    pub running: bool,
}

/// Streaming, cancellable matching using nucleo's scorer. Workers select
/// bounded top-k heaps, then merge those heaps. UI publication is O(k), and
/// backspacing can reuse a bounded cache while the index stays unchanged.
pub struct FileMatcher {
    shared: Arc<Shared>,
    generation: Arc<AtomicUsize>,
    pattern: String,
    dirty: bool,
    pending: bool,
    snapshot: Arc<Snapshot>,
    cache: VecDeque<(String, Arc<Snapshot>)>,
    sender: mpsc::Sender<Completed>,
    receiver: mpsc::Receiver<Completed>,
}

#[derive(Clone)]
pub struct FileInjector(Arc<Shared>);

impl FileMatcher {
    pub fn new(recent: &[String]) -> Self {
        Self::with_notify(recent, Arc::new(|| {}))
    }

    /// Called when indexing or a background match produces work for tick.
    /// The caller should coalesce notifications (a capacity-one channel works).
    pub fn with_notify(recent: &[String], notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut ranks = HashMap::new();
        for (rank, path) in recent.iter().take(200).enumerate() {
            ranks.entry(path.clone()).or_insert(rank);
        }
        let (sender, receiver) = mpsc::channel();
        Self {
            shared: Arc::new(Shared {
                index: Mutex::default(),
                count: AtomicUsize::new(0),
                ranks,
                notify,
            }),
            generation: Arc::new(AtomicUsize::new(0)),
            pattern: String::new(),
            dirty: true,
            pending: false,
            snapshot: Arc::default(),
            cache: VecDeque::new(),
            sender,
            receiver,
        }
    }

    pub fn injector(&self) -> FileInjector {
        FileInjector(self.shared.clone())
    }

    pub fn reparse(&mut self, pattern: &str, _append: bool) {
        if pattern == self.pattern {
            return;
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pattern = pattern.to_owned();
        self.dirty = true;
        self.pending = false;
        self.snapshot = Arc::default();
    }

    pub fn tick(&mut self) -> FileMatchStatus {
        let generation = self.generation.load(Ordering::Relaxed);
        let mut changed = false;
        while let Ok(completed) = self.receiver.try_recv() {
            if completed.generation != generation {
                continue;
            }
            self.snapshot = completed.snapshot;
            self.pending = !completed.complete;
            if completed.complete {
                self.remember();
            }
            changed = true;
        }
        let count = self.item_count();
        if self.dirty
            && let Some(index) = self
                .cache
                .iter()
                .position(|(pattern, snapshot)| *pattern == self.pattern && snapshot.count == count)
        {
            let cached = self.cache.remove(index).unwrap();
            self.snapshot = cached.1.clone();
            self.cache.push_back(cached);
            self.dirty = false;
            changed = true;
        }
        if !self.pending && (self.dirty || self.snapshot.count < count) {
            let (chunks, count) = {
                let mut index = self.shared.index.lock().unwrap();
                index.flush();
                (index.chunks.clone(), index.count)
            };
            let pattern = Pattern::parse(&self.pattern, CaseMatching::Smart, Normalization::Smart);
            let cancel = self.generation.clone();
            let sender = self.sender.clone();
            let notify = self.shared.notify.clone();
            self.dirty = false;
            self.pending = true;
            rayon::spawn(move || {
                // Give the palette fresh, correctly matched results before
                // scanning the whole index. They are refined by the full pass.
                // Never report a provisional empty set as "no matches".
                if count > 32_768 {
                    let first = chunks.partition_point(|chunk| chunk[0].id < 16_384);
                    if let Some(snapshot) =
                        match_files(&chunks[..first], count, &pattern, &cancel, generation)
                        && !snapshot.paths.is_empty()
                    {
                        sender
                            .send(Completed {
                                generation,
                                snapshot: Arc::new(snapshot),
                                complete: false,
                            })
                            .ok();
                        notify();
                    }
                }
                if let Some(snapshot) = match_files(&chunks, count, &pattern, &cancel, generation) {
                    sender
                        .send(Completed {
                            generation,
                            snapshot: Arc::new(snapshot),
                            complete: true,
                        })
                        .ok();
                    notify();
                }
            });
        }
        FileMatchStatus {
            changed,
            running: self.pending,
        }
    }

    fn remember(&mut self) {
        self.cache.retain(|(pattern, _)| *pattern != self.pattern);
        self.cache
            .push_back((self.pattern.clone(), self.snapshot.clone()));
        if self.cache.len() > CACHED_QUERIES {
            self.cache.pop_front();
        }
    }

    pub fn item_count(&self) -> usize {
        self.shared.count.load(Ordering::Acquire)
    }
    /// Number of result rows available (bounded by MAX_FILE_RESULTS).
    pub fn matched_count(&self) -> usize {
        self.snapshot.paths.len()
    }
    /// Includes matching paths below the visible result limit.
    pub fn total_matches(&self) -> usize {
        self.snapshot.matched
    }
    pub fn is_running(&self) -> bool {
        self.pending || self.dirty
    }
    pub fn item(&self, index: usize) -> Option<&str> {
        self.snapshot.paths.get(index).map(AsRef::as_ref)
    }
}

impl Drop for FileMatcher {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl FileInjector {
    pub fn push(&self, path: String) {
        // A conservative presence filter; non-ASCII paths bypass it so
        // Unicode normalization and smart-case semantics stay with nucleo.
        let ascii_mask = if path.is_ascii() {
            path.bytes().fold(0, |mask, byte| {
                mask | (1 << (byte.to_ascii_lowercase() & 63))
            })
        } else {
            u64::MAX
        };
        let text: Utf32String = path.as_str().into();
        let len = text.len();
        let recent = self.0.ranks.get(&path).copied().unwrap_or(usize::MAX);
        let notify = {
            let mut index = self.0.index.lock().unwrap();
            let id = index.count;
            index.pending.push(Candidate {
                path: path.into(),
                text,
                len,
                id,
                recent,
                ascii_mask,
            });
            index.count += 1;
            if index.pending.len() >= CHUNK_SIZE {
                index.flush();
            }
            self.0.count.store(index.count, Ordering::Release);
            index.count == 1 || index.count.is_multiple_of(CHUNK_SIZE)
        };
        if notify {
            (self.0.notify)();
        }
    }

    /// Wake the consumer after the final partial batch of a streaming walk.
    pub fn finish(&self) {
        (self.0.notify)();
    }
}

// Larger is better. Preserve nucleo's score/length/insertion-order ranking,
// with the same MRU-first group used by the palette before this optimization.
type Rank = (Reverse<usize>, u32, Reverse<usize>, Reverse<usize>);
type Best = BinaryHeap<Reverse<Rank>>;

fn retain_best(best: &mut Best, rank: Rank) {
    if best.len() < MAX_FILE_RESULTS {
        best.push(Reverse(rank));
    } else if best.peek().is_some_and(|worst| rank > worst.0) {
        *best.peek_mut().unwrap() = Reverse(rank);
    }
}

fn match_files(
    chunks: &[Arc<[Candidate]>],
    count: usize,
    pattern: &Pattern,
    cancel: &AtomicUsize,
    generation: usize,
) -> Option<Snapshot> {
    let required = pattern
        .atoms
        .iter()
        .filter(|atom| !atom.negative)
        .flat_map(|atom| atom.needle_text().chars())
        .filter(char::is_ascii)
        .fold(0u64, |mask, ch| {
            mask | (1 << ((ch as u8).to_ascii_lowercase() & 63))
        });
    let group_size = chunks
        .len()
        .div_ceil(rayon::current_num_threads().max(1) * 2)
        .max(1);
    let groups: Vec<_> = chunks
        .par_chunks(group_size)
        .map_init(
            || Matcher::new(Config::DEFAULT.match_paths()),
            |matcher, chunks| {
                let mut best = Best::with_capacity(MAX_FILE_RESULTS);
                let mut matched = 0;
                for chunk in chunks {
                    for candidate in chunk.iter() {
                        if candidate.id % 256 == 0 && cancel.load(Ordering::Relaxed) != generation {
                            return None;
                        }
                        if candidate.ascii_mask & required != required {
                            continue;
                        }
                        if let Some(score) = pattern.score(candidate.text.slice(..), matcher) {
                            matched += 1;
                            retain_best(
                                &mut best,
                                (
                                    Reverse(candidate.recent),
                                    score,
                                    Reverse(candidate.len),
                                    Reverse(candidate.id),
                                ),
                            );
                        }
                    }
                }
                Some((best, matched))
            },
        )
        .collect();
    if cancel.load(Ordering::Relaxed) != generation {
        return None;
    }
    let mut best = Best::with_capacity(MAX_FILE_RESULTS);
    let mut matched = 0;
    for group in groups {
        let (group, count) = group?;
        matched += count;
        for Reverse(rank) in group {
            retain_best(&mut best, rank);
        }
    }
    let mut ranks: Vec<_> = best.into_iter().map(|Reverse(rank)| rank).collect();
    ranks.sort_unstable_by(|a, b| b.cmp(a));
    let paths = ranks
        .into_iter()
        .map(|(_, _, _, Reverse(id))| {
            let index = chunks.partition_point(|chunk| chunk[0].id <= id) - 1;
            chunks[index][id - chunks[index][0].id].path.clone()
        })
        .collect();
    Some(Snapshot {
        paths,
        matched,
        count,
    })
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

    #[test]
    fn bounded_results_agree_with_exhaustive_scoring_and_keep_late_matches() {
        let paths: Vec<String> = (0..1500)
            .map(|i| format!("src/component-{i}/core.rs"))
            .chain(["core.rs".into(), "src/café.rs".into(), "src/CAFÉ.rs".into()])
            .collect();
        let mut matcher = FileMatcher::new(&[]);
        for path in &paths {
            matcher.injector().push(path.clone());
        }
        for query in [
            "core",
            "café",
            "CA",
            "!component",
            "^src core",
            "'core.rs$",
            "",
        ] {
            matcher.reparse(query, false);
            settle(&mut matcher);
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut scorer = Matcher::new(Config::DEFAULT.match_paths());
            let mut all: Vec<_> = paths
                .iter()
                .enumerate()
                .filter_map(|(id, path)| {
                    let text: Utf32String = path.as_str().into();
                    pattern
                        .score(text.slice(..), &mut scorer)
                        .map(|score| (Reverse(score), text.len(), id))
                })
                .collect();
            all.sort_unstable();
            assert_eq!(matcher.total_matches(), all.len(), "{query}");
            let expected: Vec<_> = all
                .iter()
                .take(MAX_FILE_RESULTS)
                .map(|(_, _, id)| paths[*id].as_str())
                .collect();
            let actual: Vec<_> = (0..matcher.matched_count())
                .map(|i| matcher.item(i).unwrap())
                .collect();
            assert_eq!(actual, expected, "{query}");
        }
    }

    #[test]
    fn stale_jobs_and_cached_queries_cannot_hide_new_paths() {
        let mut matcher = FileMatcher::new(&[]);
        for i in 0..40_000 {
            matcher.injector().push(format!("src/{i}.rs"));
        }
        matcher.reparse("src", false);
        matcher.tick();
        matcher.reparse("unique", false);
        matcher.injector().push("unique.rs".into());
        settle(&mut matcher);
        assert_eq!(matcher.total_matches(), 1);
        assert_eq!(matcher.item(0), Some("unique.rs"));
        matcher.reparse("src", false);
        settle(&mut matcher);
        matcher.injector().push("unique-new.rs".into());
        matcher.reparse("unique", false);
        settle(&mut matcher);
        assert_eq!(
            matcher.total_matches(),
            2,
            "invalidate cached results after injection"
        );
    }
}
