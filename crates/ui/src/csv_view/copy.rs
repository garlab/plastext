use super::selection::Selection;
use plastext_assets::csv::CsvRow;
use std::io;

pub(super) const MAX_COPY: usize = 64_000_000;

/// Stream selected records through a bounded clipboard buffer. Page reads run
/// on a worker and do not populate or displace the view's visible-page cache.
pub(super) fn selected_text(
    selection: &Selection,
    count: u64,
    mut page: impl FnMut(u64, usize) -> io::Result<Vec<CsvRow>>,
    cancelled: impl Fn() -> bool,
    limit: usize,
) -> io::Result<String> {
    let single_cell = selection.text_range().is_some_and(|range| {
        (range.start.row, range.start.column) == (range.end.row, range.end.column)
    });
    let mut output = String::new();
    let mut first_row = true;
    for range in selection.rows(count) {
        let mut number = range.start;
        while number < range.end {
            if cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let requested = (range.end - number).min(64) as usize;
            let rows = page(number, requested)?;
            if rows.len() != requested {
                return Err(io::Error::other("the CSV changed while copying"));
            }
            for row in rows {
                let cells = row.cells.map_err(|_| {
                    io::Error::other("this record cannot be decoded; copy it from Source instead")
                })?;
                if !first_row {
                    append(&mut output, "\n", limit)?;
                }
                first_row = false;
                let mut first_cell = true;
                for (column, cell) in cells.iter().enumerate() {
                    let range = if selection.text_range().is_some() {
                        let Some(range) = selection.cell_range(number, column, cell.len()) else {
                            continue;
                        };
                        range
                    } else {
                        0..cell.len()
                    };
                    let cell = cell
                        .get(range)
                        .ok_or_else(|| io::Error::other("the CSV changed while copying"))?;
                    if !first_cell {
                        append(&mut output, "\t", limit)?;
                    }
                    first_cell = false;
                    if single_cell {
                        append(&mut output, cell, limit)?;
                    } else {
                        append_field(&mut output, cell, limit)?;
                    }
                }
                number += 1;
            }
        }
    }
    Ok(output)
}

fn append(output: &mut String, value: &str, limit: usize) -> io::Result<()> {
    if value.len() > limit.saturating_sub(output.len()) {
        return Err(io::Error::other(
            "the selection is too large to copy (over 64 MB)",
        ));
    }
    output.push_str(value);
    Ok(())
}

fn append_field(output: &mut String, value: &str, limit: usize) -> io::Result<()> {
    if value.contains(['\t', '\r', '\n', '"']) {
        append(output, "\"", limit)?;
        let mut parts = value.split('"').peekable();
        while let Some(part) = parts.next() {
            append(output, part, limit)?;
            if parts.peek().is_some() {
                append(output, "\"\"", limit)?;
            }
        }
        append(output, "\"", limit)
    } else {
        append(output, value, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::super::selection::Position;
    use super::*;
    use plastext_assets::csv::CsvIndex;
    use plastext_buffer::FileSource;
    use std::sync::{Arc, atomic::AtomicBool};

    fn copy(csv: &str, selection: Selection) -> String {
        let source = Arc::new(FileSource::from_bytes(csv.as_bytes().to_vec()));
        let index = CsvIndex::start(
            source.clone(),
            b',',
            Arc::new(AtomicBool::new(false)),
            Arc::new(|| {}),
        );
        selected_text(
            &selection,
            3,
            |first, count| index.page(&source, b',', first, count),
            || false,
            MAX_COPY,
        )
        .unwrap()
    }

    #[test]
    fn copy_rows_as_tsv_with_quoted_multiline_and_embedded_quotes() {
        let csv = "name,note\nAlice,\"line one\nline \"\"two\"\"\"\nBob,\"a\tb\"\n";
        let mut selection = Selection::None;
        selection.select_row(1, false, false);
        selection.select_row(2, true, false);
        assert_eq!(
            copy(csv, selection),
            "Alice\t\"line one\nline \"\"two\"\"\"\nBob\t\"a\tb\""
        );
        assert!(copy(csv, Selection::All).starts_with("name\tnote\n"));
    }

    #[test]
    fn partial_cell_copy_preserves_original_unicode_and_newlines() {
        let csv = "name,note\nAlice,\"é\n界\"\nBob,last\n";
        let selection = Selection::Text {
            anchor: Position {
                row: 1,
                column: 1,
                byte: 2,
            },
            head: Position {
                row: 1,
                column: 1,
                byte: 6,
            },
        };
        assert_eq!(copy(csv, selection), "\n界");
        let selection = Selection::Text {
            anchor: Position {
                row: 2,
                column: 0,
                byte: 2,
            },
            head: Position {
                row: 1,
                column: 1,
                byte: 3,
            },
        };
        assert_eq!(copy(csv, selection), "界\nBo");
    }

    #[test]
    fn copy_nonadjacent_rows_in_file_order() {
        let mut selection = Selection::None;
        selection.select_row(2, false, false);
        selection.select_row(0, false, true);
        assert_eq!(copy("a,b\nc,d\ne,f\n", selection), "a\tb\ne\tf");
    }

    #[test]
    fn copy_crosses_page_boundaries_without_losing_multiline_records() {
        let csv: String = (0..200).map(|i| format!("{i},\"line\n{i}\"\n")).collect();
        let mut selection = Selection::None;
        selection.select_row(73, false, false);
        selection.select_row(170, true, false);
        let expected = (73..=170)
            .map(|i| format!("{i}\t\"line\n{i}\""))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(copy(&csv, selection), expected);
    }

    #[test]
    fn malformed_or_missing_records_fail_instead_of_copying_partial_results() {
        let malformed = selected_text(
            &Selection::All,
            1,
            |_, _| {
                Ok(vec![CsvRow {
                    source: 0..0,
                    cells: Err("too many columns".into()),
                }])
            },
            || false,
            MAX_COPY,
        );
        assert!(malformed.unwrap_err().to_string().contains("Source"));
        let missing = selected_text(
            &Selection::All,
            2,
            |_, _| Ok(Vec::new()),
            || false,
            MAX_COPY,
        );
        assert!(missing.unwrap_err().to_string().contains("changed"));
    }

    #[test]
    fn large_copy_is_paged_cancellable_and_never_returns_truncated_text() {
        let mut calls = 0;
        let result = selected_text(
            &Selection::All,
            20_000_000,
            |_, count| {
                assert!(count <= 64);
                calls += 1;
                Ok(vec![
                    CsvRow {
                        source: 0..0,
                        cells: Ok(vec!["data".into()])
                    };
                    count
                ])
            },
            || false,
            400,
        );
        assert!(result.is_err());
        assert_eq!(calls, 2);
        let result = selected_text(
            &Selection::All,
            20_000_000,
            |_, _| panic!("cancelled copy read data"),
            || true,
            MAX_COPY,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }
}
