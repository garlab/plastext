use plastext_search::{FileMatches, path_order};
use std::{collections::BTreeMap, ffi::OsString};

/// File indices remain stable while their display order changes as results arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    File(usize),
    Line(usize, usize),
}

#[derive(Default)]
pub(super) struct Results {
    pub files: Vec<FileMatches>,
    pub rows: Vec<Row>,
    pub lines: usize,
    // Alphabetical folder/file order, ignoring case. Original paths break ties
    // consistently on case-sensitive filesystems and for non-Unicode filenames.
    order: BTreeMap<Vec<(OsString, OsString)>, usize>,
}

impl Results {
    pub fn extend(&mut self, files: impl IntoIterator<Item = FileMatches>) {
        for file in files {
            let key = path_order(&file.path);
            self.lines += file.lines.len();
            if let Some(&index) = self.order.get(&key) {
                self.files[index].lines.extend(file.lines);
            } else {
                self.order.insert(key, self.files.len());
                self.files.push(file);
            }
        }
        // Rebuild once per received batch, never once per rendered row/frame.
        self.rows.clear();
        for &index in self.order.values() {
            self.rows.push(Row::File(index));
            self.rows
                .extend((0..self.files[index].lines.len()).map(|line| Row::Line(index, line)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_search::LineMatch;
    use std::path::PathBuf;

    fn file(path: &str) -> FileMatches {
        FileMatches {
            path: path.into(),
            lines: [2, 10]
                .map(|line| LineMatch {
                    line,
                    text: "match".into(),
                    matches: std::iter::once(0..5).collect(),
                })
                .to_vec(),
        }
    }

    fn paths(results: &Results) -> Vec<&str> {
        results
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::File(id) => results.files[*id].path.to_str(),
                Row::Line(..) => None,
            })
            .collect()
    }

    #[test]
    fn streaming_order_does_not_depend_on_worker_completion_or_batch_size() {
        let names = ["src/z.rs", "README.md", "docs/a.md", "src/A.rs", "src/a.rs"];
        let mut streamed = Results::default();
        for name in names {
            streamed.extend([file(name)]);
            let displayed = paths(&streamed);
            let mut expected = displayed.clone();
            expected.sort_by_key(|path| (path.to_lowercase(), *path));
            assert_eq!(displayed, expected);
        }
        let mut batch = Results::default();
        batch.extend(names.into_iter().rev().map(file));
        assert_eq!(paths(&streamed), paths(&batch));
        assert_eq!(
            paths(&streamed),
            ["docs/a.md", "README.md", "src/A.rs", "src/a.rs", "src/z.rs"]
        );
        assert_eq!(streamed.lines, 10);
        for group in streamed.rows.as_chunks::<3>().0 {
            let Row::File(id) = group[0] else {
                panic!("missing file header")
            };
            assert_eq!(group[1..], [Row::Line(id, 0), Row::Line(id, 1)]);
            assert_eq!(streamed.files[id].lines[0].line, 2);
            assert_eq!(streamed.files[id].lines[1].line, 10);
        }
    }

    #[test]
    fn newly_inserted_files_preserve_existing_row_identity() {
        let mut results = Results::default();
        results.extend([file("z.rs")]);
        let anchor = results.rows[1];
        results.extend([file("a.rs")]);
        assert_eq!(results.rows.iter().position(|row| *row == anchor), Some(4));
        let Row::Line(id, line) = anchor else {
            panic!("not a match")
        };
        assert_eq!(results.files[id].path, PathBuf::from("z.rs"));
        assert_eq!(results.files[id].lines[line].line, 2);
    }

    #[test]
    fn continuation_appends_to_existing_file_without_moving_rows() {
        let mut results = Results::default();
        results.extend([file("a.rs")]);
        let previous = results.rows.clone();
        let mut next = file("a.rs");
        next.lines[0].line = 20;
        next.lines[1].line = 30;
        results.extend([next, file("b.rs")]);
        assert_eq!(results.files.len(), 2);
        assert_eq!(&results.rows[..previous.len()], previous);
        assert_eq!(
            results.files[0]
                .lines
                .iter()
                .map(|line| line.line)
                .collect::<Vec<_>>(),
            [2, 10, 20, 30]
        );
        assert_eq!(results.lines, 6);
    }

    #[test]
    fn many_results_keep_folder_groups_and_line_order() {
        let mut results = Results::default();
        for start in (0..5000).step_by(64) {
            results.extend(
                (start..(start + 64).min(5000)).map(|i| file(&format!("dir/{:04}.txt", 4999 - i))),
            );
        }
        assert_eq!(results.lines, 10_000);
        assert_eq!(results.rows.len(), 15_000);
        let paths = paths(&results);
        assert_eq!(paths.first(), Some(&"dir/0000.txt"));
        assert_eq!(paths.last(), Some(&"dir/4999.txt"));
        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
