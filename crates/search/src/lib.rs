//! Quick Open and project-wide content search (spec §5.6).
//!
//! So far: [`find_all`], which finds every occurrence of a query in a file
//! of any size, and [`search_project`], which searches a whole project.

mod project;

use std::{
    io,
    ops::{ControlFlow, Range},
    sync::atomic::{AtomicBool, Ordering},
};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use plastext_buffer::{ByteSource as _, FileSource};

pub use project::{FileMatches, LineMatch, ProjectQuery, SearchOptions, search_project};

/// Reads this much at a time, so matches reach the caller as the scan goes.
const CHUNK: u64 = 4 * 1024 * 1024;

/// A literal query with smart case: all-lowercase queries match any case
/// (ASCII only), others match exactly.
pub struct Query {
    matcher: AhoCorasick,
    len: usize,
}

impl Query {
    /// `None` for an empty query.
    pub fn new(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let case_insensitive = !text.chars().any(char::is_uppercase);
        let matcher = AhoCorasickBuilder::new()
            .ascii_case_insensitive(case_insensitive)
            .match_kind(MatchKind::LeftmostFirst)
            .build([text])
            .ok()?;
        Some(Self {
            matcher,
            len: text.len(),
        })
    }
}

/// Finds every non-overlapping occurrence of `query` in `source`, front to
/// back, passing each batch of matches to `found` as the scan goes. Stops
/// early once `cancelled` is set. Blocks: call it off the main thread.
pub fn find_all(
    source: &FileSource,
    query: &Query,
    cancelled: &AtomicBool,
    mut found: impl FnMut(Vec<Range<u64>>),
) -> io::Result<()> {
    // Each window repeats the end of the previous one, so matches that
    // straddle a boundary are found too.
    let overlap = query.len.saturating_sub(1) as u64;
    let len = source.len();
    let mut window_start = 0;
    let mut window = Vec::new();
    let mut last_end = 0;
    while window_start < len && !cancelled.load(Ordering::Relaxed) {
        let read_from = window_start + window.len() as u64;
        let read_to = (window_start + CHUNK).max(read_from).min(len);
        source.scan(read_from..read_to, |chunk| {
            window.extend_from_slice(chunk);
            ControlFlow::Continue(())
        })?;
        let matches: Vec<Range<u64>> = query
            .matcher
            .find_iter(&window)
            .map(|found| window_start + found.start() as u64..window_start + found.end() as u64)
            .filter(|found| found.start >= last_end)
            .collect();
        if let Some(last) = matches.last() {
            last_end = last.end;
        }
        if !matches.is_empty() {
            found(matches);
        }
        if read_to == len {
            break;
        }
        let keep = (overlap as usize).min(window.len());
        window.drain(..window.len() - keep);
        window_start = read_to - keep as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &[u8], query: &str) -> Vec<Range<u64>> {
        let source = FileSource::from_bytes(text.to_vec());
        let query = Query::new(query).unwrap();
        let mut all = Vec::new();
        find_all(&source, &query, &AtomicBool::new(false), |matches| {
            all.extend(matches)
        })
        .unwrap();
        all
    }

    #[test]
    fn finds_with_smart_case() {
        assert_eq!(find(b"Foo foo FOO", "foo"), [0..3, 4..7, 8..11]);
        assert_eq!(find(b"Foo foo FOO", "Foo"), vec![0..3]);
        assert_eq!(find(b"aaaa", "aa"), [0..2, 2..4]);
        assert!(Query::new("").is_none());
    }

    #[test]
    fn finds_matches_across_chunk_boundaries() {
        let mut text = vec![b'.'; 3 * CHUNK as usize];
        let at = |n: usize| n * CHUNK as usize - 2;
        for start in [5, at(1), at(2)] {
            text[start..start + 5].copy_from_slice(b"needl");
        }
        text[at(1) + 5] = b'e';
        let matches = find(&text, "needle");
        let straddling = at(1) as u64;
        assert_eq!(matches, vec![straddling..straddling + 6]);

        let matches = find(&text, "needl");
        let expected: Vec<Range<u64>> = [5, at(1), at(2)]
            .iter()
            .map(|&s| s as u64..s as u64 + 5)
            .collect();
        assert_eq!(matches, expected);
    }

    #[test]
    fn stops_when_cancelled() {
        let source = FileSource::from_bytes(vec![b'a'; 3 * CHUNK as usize]);
        let query = Query::new("a").unwrap();
        let mut batches = 0;
        find_all(&source, &query, &AtomicBool::new(true), |_| batches += 1).unwrap();
        assert_eq!(batches, 0);
    }
}
