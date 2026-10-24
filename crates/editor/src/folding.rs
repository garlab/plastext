//! Fold visibility and the indentation fallback for files without a syntax tree.

use std::{collections::BTreeMap, ops::Range};

use memchr::memchr_iter;
use plastext_buffer::{ByteSource, ReadError, rows};
use plastext_syntax::Fold;

/// How far below its first line a fold's end is looked for.
const MAX_FOLD_SCAN: u64 = 16 * 1024 * 1024;

/// The indentation of `line` in columns, or `None` if it's blank.
pub(crate) fn indentation(line: &[u8], tab_size: usize) -> Option<usize> {
    let tab_size = tab_size.max(1);
    let mut width = 0;
    for &byte in line {
        match byte {
            b' ' => width += 1,
            b'\t' => width += tab_size - width % tab_size,
            b'\n' | b'\r' => return None,
            _ => return Some(width),
        }
    }
    None
}

/// What folding the line starting at `line_start` would hide: the lines
/// below it indented deeper, through the last non-blank one. `None` if the
/// next non-blank line isn't indented deeper.
pub(crate) fn fold_range(
    source: &impl ByteSource,
    line_start: u64,
    tab_size: usize,
) -> Result<Option<Range<u64>>, ReadError> {
    let window_end = source.len().min(line_start + MAX_FOLD_SCAN);
    let window = source.bytes(line_start..window_end)?;
    let mut line_ends = memchr_iter(b'\n', &window)
        .map(|i| i + 1)
        .chain([window.len()]);
    let Some(header_end) = line_ends.next() else {
        return Ok(None);
    };
    let Some(header) = indentation(&window[..header_end], tab_size) else {
        return Ok(None);
    };
    let mut start = header_end;
    let mut last_deeper = None;
    for end in line_ends {
        if end <= start {
            break;
        }
        match indentation(&window[start..end], tab_size) {
            None => {}
            Some(indent) if indent > header => last_deeper = Some(end),
            Some(_) => break,
        }
        start = end;
    }
    let to_absolute = |offset: usize| line_start + offset as u64;
    Ok(last_deeper.map(|end| to_absolute(header_end)..to_absolute(end)))
}

/// The folded ranges of a view. Folds may nest; what's hidden is their union.
#[derive(Default)]
pub(crate) struct Folds {
    /// Each fold's hidden range, by start.
    folds: BTreeMap<u64, u64>,
    /// The union of the folds, merged and sorted.
    hidden: Vec<Range<u64>>,
}

impl Folds {
    /// Whether a fold hides what follows the line ending at `line_end`.
    pub fn is_folded(&self, line_end: u64) -> bool {
        self.folds.contains_key(&line_end)
    }

    pub fn fold(&mut self, range: Range<u64>) {
        self.folds.insert(range.start, range.end);
        self.merge();
    }

    pub fn fold_many(&mut self, ranges: impl IntoIterator<Item = Range<u64>>) {
        self.folds
            .extend(ranges.into_iter().map(|range| (range.start, range.end)));
        self.merge();
    }

    pub fn unfold(&mut self, line_end: u64) {
        self.folds.remove(&line_end);
        self.merge();
    }

    pub fn unfold_all(&mut self) {
        self.folds.clear();
        self.hidden.clear();
    }

    /// Removes the folds hiding `offset`. Returns whether there were any.
    pub fn reveal(&mut self, offset: u64) -> bool {
        let before = self.folds.len();
        self.folds
            .retain(|&start, &mut end| !(start..end).contains(&offset));
        self.merge();
        self.folds.len() != before
    }

    /// The hidden range containing `offset`, if it's hidden.
    pub fn hidden_at(&self, offset: u64) -> Option<Range<u64>> {
        let index = self.hidden.partition_point(|range| range.end <= offset);
        self.hidden
            .get(index)
            .filter(|range| range.start <= offset)
            .cloned()
    }

    fn merge(&mut self) {
        self.hidden.clear();
        for (&start, &end) in &self.folds {
            match self.hidden.last_mut() {
                Some(last) if start <= last.end => last.end = last.end.max(end),
                _ => self.hidden.push(start..end),
            }
        }
    }
}

/// What the gutter shows for a row's fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FoldMark {
    /// The lines below it are folded away.
    Folded,
    /// It can fold the deeper lines below it.
    Foldable,
}

/// The fold mark of each visible row, given as (start, bytes, starts a line).
/// Whether a row can fold is judged from the visible rows below it, so the
/// last visible lines may miss their mark.
pub(crate) fn fold_marks(
    rows: &[(u64, bytes::Bytes, bool)],
    folds: &Folds,
    tab_size: usize,
    syntax: Option<&[Fold]>,
) -> Vec<Option<FoldMark>> {
    let indents: Vec<Option<usize>> = rows
        .iter()
        .map(|(_, bytes, starts_line)| {
            if *starts_line {
                indentation(bytes, tab_size)
            } else {
                None
            }
        })
        .collect();
    rows.iter()
        .enumerate()
        .map(|(i, (start, bytes, starts_line))| {
            let end = start + bytes.len() as u64;
            if !bytes.ends_with(b"\n") {
                return None;
            }
            if folds.is_folded(end) {
                return Some(FoldMark::Folded);
            }
            if let Some(syntax) = syntax {
                return (*starts_line
                    && syntax
                        .binary_search_by_key(start, |fold| fold.header)
                        .is_ok())
                .then_some(FoldMark::Foldable);
            }
            let indent = indents[i].filter(|_| *starts_line)?;
            // The next non-blank line, if it directly follows.
            let mut expected = end;
            for (j, (next_start, next_bytes, _)) in rows.iter().enumerate().skip(i + 1) {
                if *next_start != expected {
                    return None;
                }
                if let Some(next_indent) = indents[j] {
                    return (next_indent > indent).then_some(FoldMark::Foldable);
                }
                expected = next_start + next_bytes.len() as u64;
            }
            None
        })
        .collect()
}

/// The visible row after `row`, skipping folded ranges.
pub(crate) fn next_visible_row(
    source: &impl ByteSource,
    folds: &Folds,
    row: u64,
) -> Result<Option<u64>, ReadError> {
    let end = rows::row_end(source, row)?;
    let next = folds.hidden_at(end).map_or(end, |hidden| hidden.end);
    Ok((next < source.len()).then_some(next))
}

/// The visible row before `row`, skipping folded ranges.
pub(crate) fn previous_visible_row(
    source: &impl ByteSource,
    folds: &Folds,
    row: u64,
) -> Result<Option<u64>, ReadError> {
    let Some(previous) = rows::previous_row_start(source, row)? else {
        return Ok(None);
    };
    match folds.hidden_at(previous) {
        // Folds start after a line break, and merged folds never touch, so
        // the byte before a hidden range is visible.
        Some(hidden) => rows::row_start_at(source, hidden.start - 1).map(Some),
        None => Ok(Some(previous)),
    }
}

/// Up to `count` visible rows from `start`, like [`rows::rows`] but skipping
/// folded ranges.
pub(crate) fn visible_rows(
    source: &impl ByteSource,
    folds: &Folds,
    start: u64,
    count: usize,
) -> (Vec<Range<u64>>, Option<ReadError>) {
    let mut visible = Vec::with_capacity(count.min(256));
    let mut row = start;
    while visible.len() < count {
        let end = match rows::row_end(source, row) {
            Ok(end) => end,
            Err(err) => return (visible, Some(err)),
        };
        visible.push(row..end);
        match next_visible_row(source, folds, row) {
            Ok(Some(next)) => row = next,
            Ok(None) => break,
            Err(err) => return (visible, Some(err)),
        }
    }
    (visible, None)
}

#[cfg(test)]
mod tests {
    use plastext_buffer::FileSource;

    use super::*;

    const CODE: &str =
        "fn main() {\n    let x = 1;\n\n    if x {\n        go();\n    }\n}\n\nfn other() {}\n";

    fn line_start(text: &str, line: usize) -> u64 {
        text.split_inclusive('\n')
            .take(line)
            .map(str::len)
            .sum::<usize>() as u64
    }

    #[test]
    fn measures_indentation() {
        assert_eq!(indentation(b"    x", 4), Some(4));
        assert_eq!(indentation(b"\t  x", 4), Some(6));
        assert_eq!(indentation(b"  \t x", 4), Some(5));
        assert_eq!(indentation(b"   \n", 4), None);
        assert_eq!(indentation(b"", 4), None);
    }

    #[test]
    fn folds_the_deeper_block_below_a_line() {
        let source = FileSource::from_bytes(CODE);
        let fold = |line| fold_range(&source, line_start(CODE, line), 4).unwrap();
        // `fn main() {` hides its body, blank line included, but not `}`.
        assert_eq!(fold(0), Some(line_start(CODE, 1)..line_start(CODE, 6)));
        assert_eq!(fold(3), Some(line_start(CODE, 4)..line_start(CODE, 5)));
        assert_eq!(fold(1), None, "the next line isn't deeper");
        assert_eq!(fold(2), None, "blank lines don't fold");
        assert_eq!(fold(8), None, "nothing follows");
    }

    #[test]
    fn nested_folds_merge_and_reveal() {
        let mut folds = Folds::default();
        folds.fold(10..20);
        folds.fold(12..15);
        folds.fold(30..40);
        assert_eq!(folds.hidden_at(13), Some(10..20));
        assert_eq!(folds.hidden_at(25), None);
        assert_eq!(folds.hidden_at(20), None);
        assert!(folds.is_folded(12));

        folds.unfold(10);
        assert_eq!(folds.hidden_at(11), None);
        assert_eq!(folds.hidden_at(13), Some(12..15), "the inner fold stays");

        assert!(folds.reveal(35));
        assert_eq!(folds.hidden_at(35), None);
        assert!(!folds.reveal(35));
    }

    #[test]
    fn marks_rows_that_fold() {
        let source = FileSource::from_bytes(CODE);
        let rows = |folds: &Folds| -> Vec<(u64, bytes::Bytes, bool)> {
            let (rows, _) = visible_rows(&source, folds, 0, 20);
            rows.into_iter()
                .map(|row| (row.start, source.bytes(row).unwrap(), true))
                .collect()
        };
        let mut folds = Folds::default();
        let marks = fold_marks(&rows(&folds), &folds, 4, None);
        assert_eq!(marks[0], Some(FoldMark::Foldable));
        assert_eq!(marks[1], None);
        assert_eq!(marks[3], Some(FoldMark::Foldable));
        assert_eq!(marks[8], None, "the last line has nothing below it");

        folds.fold(fold_range(&source, 0, 4).unwrap().unwrap());
        let marks = fold_marks(&rows(&folds), &folds, 4, None);
        assert_eq!(marks[0], Some(FoldMark::Folded));
        assert_eq!(marks.len(), 4);
    }

    #[test]
    fn steps_over_folded_rows() {
        let source = FileSource::from_bytes(CODE);
        let mut folds = Folds::default();
        folds.fold(fold_range(&source, 0, 4).unwrap().unwrap());

        let (rows, err) = visible_rows(&source, &folds, 0, 10);
        assert_eq!(err, None);
        let starts: Vec<u64> = rows.iter().map(|row| row.start).collect();
        assert_eq!(
            starts,
            [
                0,
                line_start(CODE, 6),
                line_start(CODE, 7),
                line_start(CODE, 8)
            ]
        );

        let closing = line_start(CODE, 6);
        assert_eq!(previous_visible_row(&source, &folds, closing), Ok(Some(0)));
        assert_eq!(next_visible_row(&source, &folds, 0), Ok(Some(closing)));
    }

    #[test]
    fn syntax_marks_and_visibility_do_not_depend_on_indentation_or_visible_children() {
        let text = "fn main() {\nwork();\n}\nnext();\n";
        let source = FileSource::from_bytes(text);
        let language = plastext_syntax::detect(std::path::Path::new("main.rs"), b"").unwrap();
        let tree = plastext_syntax::SyntaxTree::parse(language, text.as_bytes(), || false).unwrap();
        let syntax = tree.folds(text.as_bytes()).unwrap();
        let first_row = vec![(0, source.bytes(0..12).unwrap(), true)];
        let mut folds = Folds::default();
        assert_eq!(
            fold_marks(&first_row, &folds, 4, Some(&syntax)),
            [Some(FoldMark::Foldable)]
        );
        folds.fold(syntax[0].hidden.clone());
        assert_eq!(
            fold_marks(&first_row, &folds, 4, Some(&syntax)),
            [Some(FoldMark::Folded)]
        );
        assert_eq!(next_visible_row(&source, &folds, 0), Ok(Some(22)));
        assert_eq!(previous_visible_row(&source, &folds, 22), Ok(Some(0)));

        let rows = vec![
            (0, bytes::Bytes::from_static(b"int n =\n"), true),
            (8, bytes::Bytes::from_static(b"    1;\n"), true),
        ];
        assert_eq!(
            fold_marks(&rows, &Folds::default(), 4, Some(&[])),
            [None, None],
            "a parsed continuation is not a fold"
        );
        assert_eq!(
            fold_marks(&rows, &Folds::default(), 4, None)[0],
            Some(FoldMark::Foldable),
            "indentation remains the fallback"
        );
    }
}
