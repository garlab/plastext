use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use crate::FileFilters;
use grep_matcher::Matcher as _;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use plastext_project::{TreeOptions, ordered_files};
use rayon::prelude::*;

/// Matching lines longer than this are cut down around their first match.
const MAX_LINE: usize = 240;

/// How to interpret a project search query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

/// A compiled project search, as a literal or a regex.
pub struct ProjectQuery {
    matcher: RegexMatcher,
}

impl ProjectQuery {
    /// Fails with a message for an invalid regex.
    pub fn new(text: &str, options: SearchOptions) -> Result<Self, String> {
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!options.case_sensitive)
            .fixed_strings(!options.regex)
            .word(options.whole_word)
            .build(text)
            .map_err(|err| err.to_string())?;
        Ok(Self { matcher })
    }
}

/// The matching lines of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    /// Relative to the project root.
    pub path: PathBuf,
    pub lines: Vec<LineMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMatch {
    /// 1-based.
    pub line: u64,
    /// The line for display: without surrounding whitespace, and cut down
    /// around the first match if it's long.
    pub text: String,
    /// Where the matches are in `text`.
    pub matches: Vec<Range<usize>>,
}

/// Case-insensitive relative path order, with a lossless tie-breaker.
pub fn path_order(path: &Path) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    path.components()
        .map(|component| {
            let name = component.as_os_str();
            (
                name.to_string_lossy().to_lowercase().into(),
                name.to_owned(),
            )
        })
        .collect()
}

const READ_AHEAD: usize = 8;
const FILE_BATCH: usize = 256;
const SEARCH_BUFFER_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
struct Cursor {
    offset: u64,
    line: u64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Version {
    len: u64,
    modified: Option<SystemTime>,
}
struct PendingFile {
    path: PathBuf,
    lines: VecDeque<LineMatch>,
    cursor: Option<Cursor>,
    version: Option<Version>,
    incomplete: bool,
}

/// An ordered, resumable project search. Lists each directory in sorted order
/// as it is reached, then scans only a bounded window of files ahead. Retains
/// at most 8 × 256 matching lines of read-ahead, regardless of the number of
/// pages requested. Use off the UI thread.
/// Entries in an already listed directory stay fixed until a new search.
pub struct ProjectSearch {
    root: PathBuf,
    query: ProjectQuery,
    paths: Box<dyn Iterator<Item = PathBuf> + Send>,
    exhausted: bool,
    filters: FileFilters,
    pending: VecDeque<PendingFile>,
    incomplete: usize,
}

impl ProjectSearch {
    pub fn new(
        root: &Path,
        options: &TreeOptions,
        query: ProjectQuery,
        filters: &FileFilters,
        cancelled: &AtomicBool,
    ) -> Self {
        let filters = filters.clone();
        let paths = ordered_files(root, options);
        Self {
            root: root.to_owned(),
            query,
            paths: Box::new(paths),
            filters,
            exhausted: cancelled.load(Ordering::Relaxed),
            pending: VecDeque::new(),
            incomplete: 0,
        }
    }

    /// Files whose contents could not be completely read (permissions, I/O or
    /// the bounded line buffer). Binary files are intentionally excluded.
    pub fn incomplete_files(&self) -> usize {
        self.incomplete
    }

    /// Emits up to `limit` more matching lines in path/line order. A file can
    /// arrive in several batches. Returns whether unsearched work remains;
    /// proving that there are no further matches would require scanning it all.
    pub fn next_page(
        &mut self,
        limit: usize,
        cancelled: &AtomicBool,
        mut found: impl FnMut(FileMatches),
    ) -> Result<bool, String> {
        let mut remaining = limit;
        while remaining > 0 && !cancelled.load(Ordering::Relaxed) {
            if self.pending.is_empty() {
                let mut paths = Vec::new();
                while !self.exhausted
                    && paths.len() < READ_AHEAD
                    && !cancelled.load(Ordering::Relaxed)
                {
                    if let Some(path) = self.paths.next() {
                        if self.filters.matches(&path) {
                            paths.push(path);
                        }
                    } else {
                        self.exhausted = true;
                    }
                }
                if paths.is_empty() {
                    break;
                }
                self.pending = paths
                    .into_par_iter()
                    .map(|path| {
                        let mut pending = PendingFile {
                            path,
                            lines: VecDeque::new(),
                            cursor: Some(Cursor::default()),
                            version: None,
                            incomplete: false,
                        };
                        // Unreadable files are skipped, like the project walker.
                        pending.scan(&self.root, &self.query, cancelled).ok();
                        pending
                    })
                    .collect::<Vec<_>>()
                    .into();
            }
            let file = self.pending.front_mut().unwrap();
            if file.lines.is_empty() && file.cursor.is_some() {
                file.scan(&self.root, &self.query, cancelled)?;
            }
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let count = remaining.min(file.lines.len());
            if count > 0 {
                found(FileMatches {
                    path: file.path.clone(),
                    lines: file.lines.drain(..count).collect(),
                });
                remaining -= count;
            }
            if file.lines.is_empty() && file.cursor.is_none() {
                self.incomplete += usize::from(file.incomplete);
                self.pending.pop_front();
            }
        }
        Ok(!self.exhausted || !self.pending.is_empty())
    }
}

impl PendingFile {
    fn scan(
        &mut self,
        root: &Path,
        query: &ProjectQuery,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let Some(cursor) = self.cursor.take() else {
            return Ok(());
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let path = root.join(&self.path);
        match std::fs::metadata(&path) {
            Ok(meta) if !meta.is_file() => return Ok(()),
            Err(_) => {
                self.incomplete = true;
                return Ok(());
            }
            _ => {}
        }
        let Ok(mut file) = File::open(path) else {
            self.incomplete = true;
            return Ok(());
        };
        let Ok(metadata) = file.metadata() else {
            self.incomplete = true;
            return Ok(());
        };
        if !metadata.is_file() {
            return Ok(());
        }
        let version = Version {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if self.version.is_some_and(|previous| previous != version) {
            return Err(format!(
                "{} changed during search. Run the search again.",
                self.path.display()
            ));
        }
        self.version = Some(version);
        if file.seek(SeekFrom::Start(cursor.offset)).is_err() {
            self.incomplete = true;
            return Ok(());
        }
        let mut sink = ChunkSink {
            query,
            cancelled,
            lines: &mut self.lines,
            start: cursor,
            next: None,
        };
        let result = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .bom_sniffing(false)
            .heap_limit(Some(SEARCH_BUFFER_BYTES))
            .build()
            .search_reader(&query.matcher, CancelReader { file, cancelled }, &mut sink);
        self.incomplete = result.is_err() && !cancelled.load(Ordering::Relaxed);
        self.cursor = sink.next.filter(|next| next.offset < version.len);
        Ok(())
    }
}

// Check cancellation on reads even when a very large file has no matches.
struct CancelReader<'a> {
    file: File,
    cancelled: &'a AtomicBool,
}
impl Read for CancelReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Ok(0);
        }
        self.file.read(bytes)
    }
}
struct ChunkSink<'a> {
    query: &'a ProjectQuery,
    cancelled: &'a AtomicBool,
    lines: &'a mut VecDeque<LineMatch>,
    start: Cursor,
    next: Option<Cursor>,
}
impl Sink for ChunkSink<'_> {
    type Error = io::Error;
    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> io::Result<bool> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let line = self.start.line + mat.line_number().unwrap_or(1);
        self.lines.push_back(line_match(
            &self.query.matcher,
            line,
            &String::from_utf8_lossy(mat.bytes()),
        ));
        if self.lines.len() >= FILE_BATCH {
            self.next = Some(Cursor {
                offset: self.start.offset + mat.absolute_byte_offset() + mat.bytes().len() as u64,
                line,
            });
            return Ok(false);
        }
        Ok(true)
    }
}

/// One page of ordered project search, retaining the original one-callback-per-
/// file API. Use `ProjectSearch` for streaming batches and continuation.
pub fn search_project(
    root: &Path,
    options: &TreeOptions,
    query: &ProjectQuery,
    limit: usize,
    cancelled: &AtomicBool,
    found: impl Fn(FileMatches) + Sync,
) {
    if limit == 0 || cancelled.load(Ordering::Relaxed) {
        return;
    }
    let mut search = ProjectSearch::new(
        root,
        options,
        ProjectQuery {
            matcher: query.matcher.clone(),
        },
        &FileFilters::default(),
        cancelled,
    );
    let mut current: Option<FileMatches> = None;
    search
        .next_page(limit, cancelled, |batch| {
            if current.as_ref().is_some_and(|file| file.path != batch.path) {
                found(current.take().unwrap());
            }
            match &mut current {
                Some(file) => file.lines.extend(batch.lines),
                None => current = Some(batch),
            }
        })
        .ok();
    if let Some(file) = current
        && !cancelled.load(Ordering::Relaxed)
    {
        found(file);
    }
}

fn line_match(matcher: &RegexMatcher, line_number: u64, line: &str) -> LineMatch {
    let first_match = matcher.find(line.as_bytes()).ok().flatten();
    // Show the line without surrounding whitespace, cut down to MAX_LINE
    // bytes starting a little before the first match.
    let content_start = line.len() - line.trim_start().len();
    let content_end = line.trim_end().len().max(content_start);
    let first = first_match.map_or(content_start, |found| found.start());
    let mut start = first.saturating_sub(MAX_LINE / 4).max(content_start);
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + MAX_LINE).min(content_end);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let ellipsis = if start > content_start { "…" } else { "" };
    let shift = ellipsis.len() as isize - start as isize;
    let text = format!("{ellipsis}{}", &line[start..end]);
    let mut matches = Vec::new();
    matcher
        .find_iter_at(line.as_bytes(), start, |found| {
            if found.start() >= end {
                return false;
            }
            if found.end() <= end {
                matches.push(
                    (found.start() as isize + shift) as usize
                        ..(found.end() as isize + shift) as usize,
                );
            }
            true
        })
        .ok();
    LineMatch {
        line: line_number,
        text,
        matches,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex};

    use super::*;

    fn search(root: &Path, text: &str, options: SearchOptions, limit: usize) -> Vec<FileMatches> {
        let query = ProjectQuery::new(text, options).unwrap();
        let found = Mutex::new(Vec::new());
        search_project(
            root,
            &TreeOptions::default(),
            &query,
            limit,
            &AtomicBool::new(false),
            |matches| {
                found.lock().unwrap().push(matches);
            },
        );
        let mut found = found.into_inner().unwrap();
        found.sort_by(|a, b| a.path.cmp(&b.path));
        found
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "fn foo() {}\n    let Foo = foo();\nbar\n",
        )
        .unwrap();
        fs::write(dir.path().join("notes.md"), "no match here\nfood\n").unwrap();
        fs::write(dir.path().join("image.bin"), b"foo\x00binary").unwrap();
        dir
    }

    #[test]
    fn finds_lines_with_explicit_case_matching() {
        let dir = project();
        let found = search(dir.path(), "foo", SearchOptions::default(), 100);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].path, Path::new("notes.md"));
        assert_eq!(found[1].path, Path::new("src/lib.rs"));
        let lines = &found[1].lines;
        let foo = 3..6;
        assert_eq!(
            lines[0],
            LineMatch {
                line: 1,
                text: "fn foo() {}".into(),
                matches: vec![foo]
            }
        );
        assert_eq!(
            lines[1],
            LineMatch {
                line: 2,
                text: "let Foo = foo();".into(),
                matches: vec![4..7, 10..13]
            }
        );

        let found = search(dir.path(), "Foo", SearchOptions::default(), 100);
        assert_eq!(
            found,
            search(dir.path(), "foo", SearchOptions::default(), 100)
        );

        let options = SearchOptions {
            case_sensitive: true,
            ..Default::default()
        };
        let found = search(dir.path(), "Foo", options, 100);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines.len(), 1);
        assert_eq!(found[0].lines[0].matches, vec![4..7]);
        let found = search(dir.path(), "foo", options, 100);
        assert_eq!(found[1].lines[1].matches, vec![10..13]);
    }

    #[test]
    fn supports_regexes_and_whole_words() {
        let dir = project();
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        let found = search(dir.path(), r"^f\w+$", options, 100);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines[0].text, "food");

        let options = SearchOptions {
            whole_word: true,
            ..Default::default()
        };
        let found = search(dir.path(), "foo", options, 100);
        assert!(found.iter().all(|file| file.path != Path::new("notes.md")));
        assert!(
            ProjectQuery::new(
                "(",
                SearchOptions {
                    regex: true,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn stops_at_the_limit() {
        let dir = project();
        let total: usize = search(dir.path(), "o", SearchOptions::default(), 2)
            .iter()
            .map(|file| file.lines.len())
            .sum();
        assert_eq!(total, 2);
        assert!(search(dir.path(), "o", SearchOptions::default(), 0).is_empty());
    }

    #[test]
    fn concurrent_files_share_one_exact_result_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..32 {
            fs::write(dir.path().join(format!("{i}.txt")), "match\n".repeat(40)).unwrap();
        }
        for _ in 0..8 {
            let found = search(dir.path(), "match", SearchOptions::default(), 17);
            assert_eq!(found.iter().map(|file| file.lines.len()).sum::<usize>(), 17);
            assert!(found.iter().all(|file| {
                file.lines
                    .windows(2)
                    .all(|lines| lines[0].line < lines[1].line)
            }));
        }
    }

    #[test]
    fn pages_keep_the_same_prefix_and_resume_within_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["z.txt", "B.txt", "b.txt", "a.txt"] {
            fs::write(
                dir.path().join(name),
                (0..731)
                    .map(|n| format!("{}\r\n", if n % 3 == 0 { "nothing" } else { "match é" }))
                    .collect::<String>(),
            )
            .unwrap();
        }
        for directory in ["A", "a"] {
            fs::create_dir_all(dir.path().join(directory)).unwrap();
            for name in ["z.txt", "a.txt"] {
                fs::write(dir.path().join(directory).join(name), "match\n").unwrap();
            }
        }
        let expected = search(dir.path(), "match", SearchOptions::default(), usize::MAX);
        let expected: Vec<_> = expected
            .into_iter()
            .flat_map(|f| f.lines.into_iter().map(move |l| (f.path.clone(), l)))
            .collect();
        let mut expected = expected;
        expected.sort_by_key(|(path, line)| (path_order(path), line.line));
        for size in [1, 17, 256, 999] {
            let cancel = AtomicBool::new(false);
            let mut session = ProjectSearch::new(
                dir.path(),
                &TreeOptions::default(),
                ProjectQuery::new("match", SearchOptions::default()).unwrap(),
                &FileFilters::default(),
                &cancel,
            );
            let mut actual = Vec::new();
            loop {
                let before = actual.len();
                let more = session
                    .next_page(size, &cancel, |file| {
                        actual.extend(file.lines.into_iter().map(|line| (file.path.clone(), line)));
                    })
                    .unwrap();
                assert!(actual.len() - before <= size);
                assert_eq!(actual, expected[..actual.len()]);
                if !more {
                    break;
                }
            }
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn cap_leaves_later_contents_unread_and_continuation_detects_edits() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..20 {
            fs::write(
                dir.path().join(format!("{n:02}.txt")),
                "match\n".repeat(600),
            )
            .unwrap();
        }
        let cancel = AtomicBool::new(false);
        let mut session = ProjectSearch::new(
            dir.path(),
            &TreeOptions::default(),
            ProjectQuery::new("match", SearchOptions::default()).unwrap(),
            &FileFilters::default(),
            &cancel,
        );
        let mut first = Vec::new();
        assert!(session.next_page(17, &cancel, |f| first.push(f)).unwrap());
        assert!(first.iter().all(|file| file.path == Path::new("00.txt")));
        assert_eq!(session.paths.by_ref().count(), 20 - READ_AHEAD);
        assert!(
            session
                .pending
                .iter()
                .map(|file| file.lines.len())
                .sum::<usize>()
                <= READ_AHEAD * FILE_BATCH
        );
        fs::write(dir.path().join("00.txt"), "changed\n").unwrap();
        assert!(
            session
                .next_page(1000, &cancel, |_| {})
                .unwrap_err()
                .contains("changed")
        );
    }

    #[test]
    fn filters_keep_project_ignore_rules_and_cancelled_pages_publish_nothing() {
        let dir = project();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".gitignore"), "notes.md\n").unwrap();
        let cancel = AtomicBool::new(false);
        let mut session = ProjectSearch::new(
            dir.path(),
            &TreeOptions::default(),
            ProjectQuery::new("o", SearchOptions::default()).unwrap(),
            &FileFilters::new("*.md, src", "").unwrap(),
            &cancel,
        );
        let mut paths = Vec::new();
        assert!(
            !session
                .next_page(100, &cancel, |f| paths.push(f.path))
                .unwrap()
        );
        assert_eq!(paths, [PathBuf::from("src/lib.rs")]);
        cancel.store(true, Ordering::Relaxed);
        session
            .next_page(10, &cancel, |_| panic!("cancelled page published"))
            .unwrap();
    }

    #[test]
    fn case_option_combines_with_regex_whole_word_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("words.txt"), "école ÉCOLE écoles\n").unwrap();
        for regex in [false, true] {
            let text = if regex { "écol[e]" } else { "école" };
            let lowercase = 0..6;
            for (case_sensitive, expected) in [
                (false, vec![lowercase.clone(), 7..13]),
                (true, vec![lowercase]),
            ] {
                let found = search(
                    dir.path(),
                    text,
                    SearchOptions {
                        case_sensitive,
                        regex,
                        whole_word: true,
                    },
                    100,
                );
                assert_eq!(found[0].lines[0].matches, expected);
            }
        }
    }

    #[test]
    fn cancelled_search_publishes_nothing() {
        let dir = project();
        let query = ProjectQuery::new("foo", SearchOptions::default()).unwrap();
        search_project(
            dir.path(),
            &TreeOptions::default(),
            &query,
            100,
            &AtomicBool::new(true),
            |_| panic!("cancelled search published results"),
        );
    }

    #[test]
    fn reports_incomplete_files_and_keeps_searching_other_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.txt"),
            "x".repeat(SEARCH_BUFFER_BYTES + 1),
        )
        .unwrap();
        fs::write(dir.path().join("b.txt"), "needle\n").unwrap();
        fs::write(dir.path().join("c.bin"), b"needle\x00binary").unwrap();
        let cancel = AtomicBool::new(false);
        let mut session = ProjectSearch::new(
            dir.path(),
            &TreeOptions::default(),
            ProjectQuery::new("needle", SearchOptions::default()).unwrap(),
            &FileFilters::default(),
            &cancel,
        );
        let mut found = Vec::new();
        assert!(
            !session
                .next_page(100, &cancel, |file| found.push(file))
                .unwrap()
        );
        assert_eq!(session.incomplete_files(), 1);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, Path::new("b.txt"));
    }

    #[test]
    fn cuts_long_lines_around_the_first_match() {
        let line = format!("{}needle{}", "x".repeat(1000), "y".repeat(1000));
        let matcher = RegexMatcherBuilder::new()
            .fixed_strings(true)
            .build("needle")
            .unwrap();
        let found = line_match(&matcher, 7, &line);
        assert!(found.text.len() <= MAX_LINE + "…".len());
        assert!(found.text.starts_with('…'));
        assert_eq!(&found.text[found.matches[0].clone()], "needle");
    }
}
