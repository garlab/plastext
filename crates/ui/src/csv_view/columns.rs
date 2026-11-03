use plastext_assets::csv::CsvRow;

pub(super) const FONT_SIZE: f32 = 14.;
const MIN_WIDTH: f32 = 64.;
const DEFAULT_WIDTH: f32 = 160.;
const MAX_AUTO_WIDTH: f32 = 560.;
const MAX_WIDTH: f32 = 4096.;
const CELL_INSET: f32 = 24.;
const SAMPLE_ROWS: usize = 64;
const SAMPLE_CHARS: usize = 256;

#[derive(Clone, Default)]
pub(super) struct Columns {
    sampled: Vec<f32>,
    overrides: Vec<Option<f32>>,
}

impl Columns {
    pub fn len(&self) -> usize {
        self.sampled.len()
    }

    pub fn ensure_count(&mut self, count: usize) {
        self.sampled.resize(count.max(self.len()), DEFAULT_WIDTH);
        self.overrides.resize(self.len(), None);
    }

    pub fn set_sampled(&mut self, widths: Vec<f32>) {
        self.ensure_count(widths.len());
        self.sampled[..widths.len()].copy_from_slice(&widths);
    }

    pub fn width(&self, column: usize) -> f32 {
        self.overrides
            .get(column)
            .copied()
            .flatten()
            .or_else(|| self.sampled.get(column).copied())
            .unwrap_or(DEFAULT_WIDTH)
    }

    pub fn total_width(&self) -> f32 {
        (0..self.len()).map(|column| self.width(column)).sum()
    }

    /// Columns intersecting a logical horizontal interval, plus the width skipped
    /// before them. Keep partial edge cells; callers clip them at paint time.
    pub fn visible(&self, left: f32, right: f32, count: usize) -> (std::ops::Range<usize>, f32) {
        if right <= left || right <= 0. {
            return (0..0, 0.);
        }
        let mut first = 0;
        let mut before = 0.;
        while first < count && before + self.width(first) <= left {
            before += self.width(first);
            first += 1;
        }
        let mut last = first;
        let mut end = before;
        while last < count && end < right {
            end += self.width(last);
            last += 1;
        }
        (first..last, before)
    }

    pub fn resize(&mut self, column: usize, width: f32) {
        self.ensure_count(column + 1);
        self.overrides[column] = Some(width.clamp(MIN_WIDTH, MAX_WIDTH));
    }

    pub fn reset(&mut self, column: usize) {
        if let Some(width) = self.overrides.get_mut(column) {
            *width = None;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ColumnDrag {
    pub column: usize,
    pub start_x: f32,
    pub start_width: f32,
}

impl ColumnDrag {
    pub fn width_at(self, x: f32, zoom: f32) -> f32 {
        // Use window coordinates so scroll clamping when the table shrinks
        // cannot feed back into the width calculation.
        (self.start_width + (x - self.start_x) / zoom).clamp(MIN_WIDTH, MAX_WIDTH)
    }
}

// Match the single-line representation used by table cells without allocating
// an expanded copy of a potentially large multiline field.
pub(super) fn display_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().flat_map(|ch| match ch {
        '\r' | '\n' => [' ', '↵', ' '].into_iter().take(3),
        _ => [ch, ' ', ' '].into_iter().take(1),
    })
}

/// Runs alongside the first page read, never while scrolling or on the UI thread.
pub(super) fn sample_widths(
    rows: &[CsvRow],
    mut advance: impl FnMut(char, bool) -> f32,
) -> Vec<f32> {
    let mut widths: Vec<f32> = Vec::new();
    for (row, record) in rows.iter().take(SAMPLE_ROWS).enumerate() {
        let Ok(cells) = &record.cells else { continue };
        widths.resize(widths.len().max(cells.len()), MIN_WIDTH);
        for (column, cell) in cells.iter().enumerate() {
            if widths[column] >= MAX_AUTO_WIDTH {
                continue;
            }
            let mut width = CELL_INSET;
            for ch in display_chars(cell).take(SAMPLE_CHARS) {
                width += advance(ch, row == 0);
                if width >= MAX_AUTO_WIDTH {
                    break;
                }
            }
            widths[column] = widths[column].max(width.min(MAX_AUTO_WIDTH)).ceil();
        }
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> CsvRow {
        CsvRow {
            source: 0..0,
            cells: Ok(cells.iter().map(|cell| cell.to_string()).collect()),
        }
    }

    #[test]
    fn samples_body_and_bold_header_with_room_for_long_identifiers() {
        let rows = [
            row(&["ID", "HEADER", "n"]),
            row(&[&"a".repeat(55), "x", "1"]),
        ];
        let widths = sample_widths(&rows, |_, header| if header { 9. } else { 8. });
        assert_eq!(widths, vec![464., 78., MIN_WIDTH]);
    }

    #[test]
    fn sampling_bounds_wide_cells_and_work_even_for_zero_width_glyphs() {
        let rows = vec![row(&[&"x".repeat(65_536)]); 100];
        let mut calls = 0;
        assert_eq!(
            sample_widths(&rows, |_, _| {
                calls += 1;
                0.
            }),
            vec![MIN_WIDTH]
        );
        assert_eq!(calls, SAMPLE_ROWS * SAMPLE_CHARS);
        assert_eq!(sample_widths(&rows, |_, _| 14.), vec![MAX_AUTO_WIDTH]);
    }

    #[test]
    fn multiline_and_unicode_use_displayed_characters() {
        assert_eq!(display_chars("a\r\nb").collect::<String>(), "a ↵  ↵ b");
        let widths = sample_widths(&[row(&["界\n界"])], |ch, _| match ch {
            '界' => 14.,
            '↵' => 10.,
            _ => 4.,
        });
        assert_eq!(widths, vec![70.]);
    }

    #[test]
    fn overrides_survive_sampling_and_duplicate_independently() {
        let mut columns = Columns::default();
        columns.resize(0, 300.);
        columns.set_sampled(vec![100., 240.]);
        columns.ensure_count(1);
        assert_eq!(columns.total_width(), 540.);
        let mut split = columns.clone();
        split.reset(0);
        split.resize(1, 200.);
        assert_eq!(split.total_width(), 300.);
        assert_eq!(columns.total_width(), 540.);
        columns.ensure_count(3);
        assert_eq!(columns.width(2), DEFAULT_WIDTH);
    }

    #[test]
    fn visible_columns_keep_partial_edges_and_their_original_positions() {
        let mut columns = Columns::default();
        columns.set_sampled(vec![64., 100., 150.]);
        assert_eq!(columns.visible(63., 165., 3), (0..3, 0.));
        assert_eq!(columns.visible(64., 164., 3), (1..2, 64.));
        assert_eq!(columns.visible(164., 314., 3), (2..3, 164.));
        assert_eq!(columns.visible(314., 900., 3), (3..3, 314.));
        assert_eq!(columns.visible(-20., 64., 3), (0..1, 0.));
        assert_eq!(columns.visible(0., 0., 3), (0..0, 0.));
        // Newly discovered ragged columns use the normal default width.
        assert_eq!(columns.visible(314., 474., 5), (3..4, 314.));
        columns.resize(0, 200.);
        assert_eq!(columns.visible(200., 300., 3), (1..2, 200.));
    }

    #[test]
    fn wide_tables_only_visit_columns_intersecting_the_viewport_at_any_zoom() {
        let mut columns = Columns::default();
        columns.ensure_count(128);
        for zoom in [0.5, 1., 2.] {
            let left = 10.5 * DEFAULT_WIDTH;
            let right = left + 800. / zoom;
            let (range, before) = columns.visible(left, right, 128);
            assert_eq!(range.start, 10);
            assert_eq!(before, 10. * DEFAULT_WIDTH);
            assert!(range.len() <= (800. / zoom / MIN_WIDTH).ceil() as usize + 2);
            for i in 0..128 {
                let intersects =
                    (i + 1) as f32 * DEFAULT_WIDTH > left && (i as f32) * DEFAULT_WIDTH < right;
                assert_eq!(range.contains(&i), intersects);
            }
        }
    }

    #[test]
    fn dragging_tracks_pointer_in_window_coordinates_at_each_zoom() {
        let drag = ColumnDrag {
            column: 0,
            start_x: 400.,
            start_width: 240.,
        };
        assert_eq!(drag.width_at(440., 1.), 280.);
        assert_eq!(drag.width_at(440., 2.), 260.);
        assert_eq!(drag.width_at(440., 0.5), 320.);
        assert_eq!(drag.width_at(400., 1.), 240.);
        assert_eq!(drag.width_at(-1000., 1.), MIN_WIDTH);
        assert_eq!(drag.width_at(10_000., 1.), MAX_WIDTH);
    }
}
