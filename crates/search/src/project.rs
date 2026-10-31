use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use grep_matcher::Matcher as _;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::Lossy};
use plastext_project::{TreeOptions, walk_files};

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

/// Searches the project at `root` (the files Quick Open lists) for `query`
/// on several threads (spec NAV-2), passing each file's matches to `found`
/// as soon as that file is done. Stops after `limit` matching lines or once
/// `cancelled` is set, and skips binary files. Blocks: call it off the main
/// thread. Files arrive in worker completion order; consumers should order them
/// for display. When capped, the retained subset depends on scan order.
pub fn search_project(
    root: &Path,
    options: &TreeOptions,
    query: &ProjectQuery,
    limit: usize,
    cancelled: &AtomicBool,
    found: impl Fn(FileMatches) + Sync,
) {
    let stop = AtomicBool::new(false);
    let matched = AtomicUsize::new(0);
    walk_files(root, options, &stop, |relative| {
        if cancelled.load(Ordering::Relaxed) || matched.load(Ordering::Relaxed) >= limit {
            stop.store(true, Ordering::Relaxed);
            return;
        }
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .build();
        let mut lines = Vec::new();
        let sink = Lossy(|line_number, line: &str| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            // Reserve a slot before retaining the line. Concurrent files must
            // not overshoot the limit or keep publishing after cancellation.
            let Ok(previous) = matched.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < limit).then(|| n + 1)
            }) else {
                return Ok(false);
            };
            lines.push(line_match(&query.matcher, line_number, line));
            Ok(previous + 1 < limit && !cancelled.load(Ordering::Relaxed))
        });
        // An unreadable file is skipped.
        searcher
            .search_path(&query.matcher, root.join(relative), sink)
            .ok();
        if matched.load(Ordering::Relaxed) >= limit {
            stop.store(true, Ordering::Relaxed);
        }
        if !lines.is_empty() && !cancelled.load(Ordering::Relaxed) {
            found(FileMatches {
                path: relative.to_owned(),
                lines,
            });
        }
    });
}

fn line_match(matcher: &RegexMatcher, line_number: u64, line: &str) -> LineMatch {
    let mut matches = Vec::new();
    matcher
        .find_iter(line.as_bytes(), |found| {
            matches.push(found.start()..found.end());
            true
        })
        .ok();
    // Show the line without surrounding whitespace, cut down to MAX_LINE
    // bytes starting a little before the first match.
    let content_start = line.len() - line.trim_start().len();
    let content_end = line.trim_end().len().max(content_start);
    let first = matches.first().map_or(content_start, |found| found.start);
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
    let matches = matches
        .into_iter()
        .filter(|found| found.start >= start && found.end <= end)
        .map(|found| (found.start as isize + shift) as usize..(found.end as isize + shift) as usize)
        .collect();
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
