//! Bounded line-oriented document search and regex replacement. Captures are
//! expanded against the same immutable bytes that produced each match.
use crate::SearchOptions;
use regex::bytes::{Regex, RegexBuilder};
use std::{
    io,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct DocumentQuery {
    regex: Regex,
    literal: bool,
    overlap: usize,
}
impl DocumentQuery {
    pub fn new(pattern: &str, options: SearchOptions) -> Result<Self, String> {
        if pattern.len() > 32768 {
            return Err("Search patterns are limited to 32 KiB".into());
        }
        let overlap = pattern.len().saturating_mul(4) + 16;
        let pattern = if options.regex {
            pattern.to_owned()
        } else {
            regex::escape(pattern)
        };
        let pattern = if options.whole_word {
            format!("\\b(?:{pattern})\\b")
        } else {
            pattern
        };
        Ok(Self {
            regex: RegexBuilder::new(&pattern)
                .case_insensitive(!options.case_sensitive)
                .multi_line(true)
                .size_limit(4 * 1024 * 1024)
                .build()
                .map_err(|e| e.to_string())?,
            literal: !options.regex,
            overlap,
        })
    }

    /// `replacement` enables capture expansion ($1, ${name}, $$); literals copy
    /// replacement text verbatim. No cache or document-sized buffer is retained.
    pub fn scan(
        &self,
        len: u64,
        mut read: impl FnMut(Range<u64>, &mut Vec<u8>) -> io::Result<()>,
        scope: Range<u64>,
        replacement: Option<&str>,
        cancel: &AtomicBool,
        mut found: impl FnMut(Range<u64>, Option<String>) -> bool,
    ) -> io::Result<()> {
        let dollars = replacement.map_or(0, |t| t.bytes().filter(|b| *b == b'$').count());
        let mut pending = Vec::new();
        let mut read_at = 0;
        let mut base = 0;
        let mut emitted_until = 0u64;
        let mut last_match_end = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let end = (read_at + 256 * 1024).min(len);
            read(read_at..end, &mut pending)?;
            read_at = end;
            let eof = end == len;
            // Literal search retains enough context for every possible UTF-8
            // case-folded match and its word boundaries, even on enormous lines.
            // Regex search is explicitly per logical line, independent of I/O chunks.
            let complete = if self.literal {
                if eof {
                    pending.len()
                } else {
                    pending.len().saturating_sub(self.overlap)
                }
            } else if eof {
                pending.len()
            } else {
                pending
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(i, b)| **b == b'\n' || **b == b'\r' && *i + 1 < pending.len())
                    .map_or(0, |(i, _)| i + 1)
            };
            let mut segments = Vec::new();
            if self.literal {
                segments.push(0..pending.len());
            } else {
                let mut start = 0;
                let mut i = 0;
                while i < complete {
                    if matches!(pending[i], b'\r' | b'\n') {
                        segments.push(start..i);
                        if pending[i] == b'\r' && pending.get(i + 1) == Some(&b'\n') {
                            i += 1;
                        }
                        start = i + 1;
                    }
                    i += 1;
                }
                if start < complete || eof {
                    segments.push(start..complete);
                }
            }
            for segment in segments {
                let bytes = &pending[segment.clone()];
                let mut cursor = if self.literal {
                    emitted_until.max(last_match_end).saturating_sub(base) as usize
                } else {
                    0
                };
                while cursor <= bytes.len() {
                    let Some(capture) = self.regex.captures_at(bytes, cursor) else {
                        break;
                    };
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let matched = capture.get(0).unwrap();
                    cursor = if matched.is_empty() {
                        matched.end() + 1
                    } else {
                        matched.end()
                    };
                    let start = segment.start + matched.start();
                    let range = base + start as u64..base + (segment.start + matched.end()) as u64;
                    if self.literal
                        && (!eof && start >= complete
                            || range.start < emitted_until
                            || range.start < last_match_end)
                    {
                        continue;
                    }
                    if range.start < scope.start || range.end > scope.end {
                        continue;
                    }
                    let value = replacement
                        .map(|template| {
                            if template.len().saturating_add(if self.literal {
                                0
                            } else {
                                dollars.saturating_mul(matched.len())
                            }) > 64 * 1024 * 1024
                            {
                                return Err(io::Error::other(
                                    "Expanded replacement may exceed 64 MiB",
                                ));
                            }
                            if self.literal {
                                return Ok(template.to_owned());
                            }
                            let mut expanded = Vec::new();
                            capture.expand(template.as_bytes(), &mut expanded);
                            String::from_utf8(expanded).map_err(|_| {
                                io::Error::other("Replacement contains invalid UTF-8 bytes")
                            })
                        })
                        .transpose()?;
                    last_match_end = range.end;
                    if !found(range, value) {
                        return Ok(());
                    }
                }
            }
            if eof {
                break;
            }
            emitted_until = base + complete as u64;
            let discard = if self.literal {
                complete.saturating_sub(4)
            } else {
                complete
            };
            pending.drain(..discard);
            base += discard as u64;
            if pending.len() > 8 * 1024 * 1024 {
                return Err(io::Error::other(
                    "Regex search encountered a line larger than 8 MiB; use literal search or a smaller file",
                ));
            }
            if base > scope.end {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flags_captures_unicode_and_zero_width_are_stable_across_chunks() {
        let input = "Foo foo food\nα = 42\n";
        let run = |pattern, options, replacement| {
            let query = DocumentQuery::new(pattern, options).unwrap();
            let mut matches = Vec::new();
            query
                .scan(
                    input.len() as u64,
                    |r, out| {
                        out.extend(&input.as_bytes()[r.start as usize..r.end as usize]);
                        Ok(())
                    },
                    0..input.len() as u64,
                    replacement,
                    &AtomicBool::new(false),
                    |r, s| {
                        matches.push((r, s));
                        true
                    },
                )
                .unwrap();
            matches
        };
        assert_eq!(
            run(
                "foo",
                SearchOptions {
                    case_sensitive: true,
                    whole_word: true,
                    regex: false
                },
                None
            )
            .len(),
            1
        );
        assert_eq!(
            run(
                "foo",
                SearchOptions {
                    whole_word: true,
                    ..Default::default()
                },
                None
            )
            .len(),
            2
        );
        let found = run(
            r"(?P<name>α) = (\d+)",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
            Some("${name}: $2 $$"),
        );
        assert_eq!(found[0].1.as_deref(), Some("α: 42 $"));
        assert!(
            !run(
                "^",
                SearchOptions {
                    regex: true,
                    ..Default::default()
                },
                Some("x")
            )
            .is_empty()
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    fn scan(query: &DocumentQuery, input: &[u8], scope: Range<u64>) -> Vec<Range<u64>> {
        let mut found = Vec::new();
        query
            .scan(
                input.len() as u64,
                |r, out| {
                    out.extend_from_slice(&input[r.start as usize..r.end as usize]);
                    Ok(())
                },
                scope,
                None,
                &AtomicBool::new(false),
                |r, _| {
                    found.push(r);
                    true
                },
            )
            .unwrap();
        found
    }
    #[test]
    fn literal_matches_across_chunks_agree_with_full_search() {
        for pattern in ["aa", "aaa", "αA", "abcab"] {
            let input = format!("{} {}", pattern.repeat(120_000), pattern).into_bytes();
            let query = DocumentQuery::new(pattern, SearchOptions::default()).unwrap();
            let expected: Vec<_> = query
                .regex
                .find_iter(&input)
                .map(|m| m.start() as u64..m.end() as u64)
                .collect();
            assert_eq!(scan(&query, &input, 0..input.len() as u64), expected);
        }
    }
    #[test]
    fn whole_words_and_regex_lines_keep_crlf_and_scope_boundaries() {
        let input = format!("{}\r\nα42\r\nx\nα9\n", "x".repeat(256 * 1024 - 1));
        let query = DocumentQuery::new(
            r"^α\d+$",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        let found = scan(&query, input.as_bytes(), 0..input.len() as u64);
        assert_eq!(found.len(), 2);
        assert_eq!(
            &input[found[0].start as usize..found[0].end as usize],
            "α42"
        );
        assert_eq!(
            scan(&query, input.as_bytes(), found[1].clone()),
            vec![found[1].clone()]
        );
        let query = DocumentQuery::new(
            "foo",
            SearchOptions {
                whole_word: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            scan(&query, "αfoo foo FOO food".as_bytes(), 0..18),
            vec![6..9, 10..13]
        );
    }
    #[test]
    fn long_literal_lines_remain_bounded_and_cancellation_stops_reads() {
        let input = vec![b'x'; 9 * 1024 * 1024];
        let query = DocumentQuery::new("missing", SearchOptions::default()).unwrap();
        assert!(scan(&query, &input, 0..input.len() as u64).is_empty());
        let cancelled = AtomicBool::new(true);
        query
            .scan(
                100,
                |_, _| panic!("cancelled read"),
                0..100,
                None,
                &cancelled,
                |_, _| panic!("cancelled result"),
            )
            .unwrap();
    }
}
