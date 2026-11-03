//! Paint-only text backgrounds. GPUI 0.2's multiline shaper splits font runs
//! at background changes; on macOS those boundaries insert ZWNJs and change
//! glyph metrics. Selection and find must never participate in shaping.
use gpui::{
    Bounds, Pixels, TextAlign, TextLayout, Window, WrappedLineLayout, fill, point, px, size,
};
use plastext_editor::Theme;
use std::ops::Range;

pub(super) fn paint(
    layout: &TextLayout,
    matches: &[Range<usize>],
    selected: Option<&Range<usize>>,
    align: TextAlign,
    theme: Theme,
    window: &mut Window,
) {
    let bounds = layout.bounds();
    let height = layout.line_height();
    let mut origin = bounds.origin;
    let mut at = 0;
    while let Some(line) = layout.line_layout_for_index(at) {
        let end = at + line.len();
        let first_match = matches.partition_point(|range| range.end <= at);
        let last_match = matches.partition_point(|range| range.start < end);
        let matches = &matches[first_match..last_match.max(first_match)];
        let selected = selected.filter(|range| range.end > at && range.start < end);
        if !matches.is_empty() || selected.is_some() {
            let geometry = Geometry::new(&line);
            let row_bounds = Bounds::new(origin, size(bounds.size.width, height));
            // Find below selection; glyphs and inline formulas paint afterwards.
            for (ranges, color) in [
                (matches, theme.find_match),
                (
                    selected.map_or(&[][..], std::slice::from_ref),
                    theme.selection,
                ),
            ] {
                for rect in geometry.rects(row_bounds, align, at, ranges) {
                    window.paint_quad(fill(rect, color));
                }
            }
        }
        origin.y += line.size(height).height;
        at = end + 1; // Hard newline; wraps remain within this layout.
    }
}

/// Existing glyph positions and wrap ends, never a second text layout.
struct Geometry {
    points: Vec<(usize, Pixels)>,
    ends: Vec<usize>,
    source_boundaries: Vec<usize>,
}

impl Geometry {
    fn new(line: &WrappedLineLayout) -> Self {
        let mut points = Vec::new();
        let mut ends = Vec::new();
        let mut wraps = line.wrap_boundaries.iter().peekable();
        for (run_ix, run) in line.unwrapped_layout.runs.iter().enumerate() {
            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                if wraps
                    .peek()
                    .is_some_and(|b| b.run_ix == run_ix && b.glyph_ix == glyph_ix)
                {
                    ends.push(points.len());
                    wraps.next();
                }
                points.push((glyph.index, glyph.position.x));
            }
        }
        ends.push(points.len());
        points.push((line.len(), line.unwrapped_layout.width));
        // Visual glyph order can differ from source order (RTL); repeated
        // indices are components of the same shaped cluster.
        let mut source_boundaries: Vec<_> = points.iter().map(|p| p.0).collect();
        source_boundaries.sort_unstable();
        source_boundaries.dedup();
        Self {
            points,
            ends,
            source_boundaries,
        }
    }

    fn rects(
        &self,
        bounds: Bounds<Pixels>,
        align: TextAlign,
        source_start: usize,
        ranges: &[Range<usize>],
    ) -> Vec<Bounds<Pixels>> {
        let mut rects: Vec<Bounds<Pixels>> = Vec::new();
        if ranges.is_empty() {
            return rects;
        }
        let mut start = 0;
        for (row, &end) in self.ends.iter().enumerate() {
            let start_x = self.points[start].1;
            let spare = bounds.size.width - (self.points[end].1 - start_x);
            let left = bounds.left()
                + match align {
                    TextAlign::Left => px(0.),
                    TextAlign::Center => spare / 2.,
                    TextAlign::Right => spare,
                };
            let top = bounds.top() + bounds.size.height * row;
            for glyph in start..end {
                let (byte, x1) = self.points[glyph];
                let x2 = self.points[glyph + 1].1;
                let next = self.source_boundaries.partition_point(|&b| b <= byte);
                let Some(&until) = self.source_boundaries.get(next) else {
                    continue;
                };
                let hit = ranges.partition_point(|r| r.end <= source_start + byte);
                if x2 <= x1
                    || !ranges
                        .get(hit)
                        .is_some_and(|r| r.start < source_start + until)
                {
                    continue;
                }
                // Color the whole cluster without splitting ligatures or
                // combining marks. Adjacent highlighted glyphs share one quad.
                let x = left + x1 - start_x;
                if let Some(previous) = rects
                    .last_mut()
                    .filter(|r| r.top() == top && r.right() == x)
                {
                    previous.size.width += x2 - x1;
                } else {
                    rects.push(Bounds::new(
                        point(x, top),
                        size(x2 - x1, bounds.size.height),
                    ));
                }
            }
            start = end;
        }
        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_center_and_right_aligned_ranges_use_existing_glyph_positions() {
        let geometry = Geometry {
            points: [
                (0, 0.),
                (1, 8.),
                (2, 17.),
                (3, 23.),
                (4, 31.),
                (5, 40.),
                (6, 45.),
            ]
            .map(|(i, x)| (i, px(x)))
            .to_vec(),
            ends: vec![3, 6],
            source_boundaries: (0..=6).collect(),
        };
        let bounds = Bounds::new(point(px(100.), px(50.)), size(px(60.), px(20.)));
        // Source offsets include previous hard lines / table cells.
        let range = 11..15;
        for (align, expected) in [
            (TextAlign::Left, [108., 100.]),
            (TextAlign::Center, [126.5, 119.]),
            (TextAlign::Right, [145., 138.]),
        ] {
            let rects = geometry.rects(bounds, align, 10, std::slice::from_ref(&range));
            assert_eq!(rects.len(), 2);
            assert_eq!(
                rects[0],
                Bounds::new(point(px(expected[0]), px(50.)), size(px(15.), px(20.)))
            );
            assert_eq!(
                rects[1],
                Bounds::new(point(px(expected[1]), px(70.)), size(px(17.), px(20.)))
            );
        }
    }

    #[test]
    fn partial_clusters_and_formula_placeholders_do_not_require_reshaping() {
        let geometry = Geometry {
            // A multibyte cluster, an ffi ligature, then a formula placeholder.
            points: vec![
                (0, px(0.)),
                (3, px(10.)),
                (6, px(22.)),
                (7, px(30.)),
                (8, px(38.)),
            ],
            ends: vec![4],
            source_boundaries: vec![0, 3, 6, 7, 8],
        };
        let bounds = Bounds::new(gpui::Point::default(), size(px(100.), px(20.)));
        let accent = geometry.rects(bounds, TextAlign::Left, 0, std::slice::from_ref(&(1..2)));
        assert_eq!(accent[0].size.width, px(10.));
        let ligature = geometry.rects(bounds, TextAlign::Left, 0, std::slice::from_ref(&(4..5)));
        assert_eq!(ligature[0].origin.x, px(10.));
        assert_eq!(ligature[0].size.width, px(12.));
        let formula = geometry.rects(bounds, TextAlign::Left, 0, std::slice::from_ref(&(6..8)));
        assert_eq!(formula[0].size.width, px(16.));
        assert!(
            geometry
                .rects(bounds, TextAlign::Left, 0, std::slice::from_ref(&(8..12)))
                .is_empty()
        );
    }

    #[test]
    fn visual_order_handles_rtl_and_repeated_cluster_indices() {
        let geometry = Geometry {
            points: vec![
                (0, px(0.)),
                (6, px(8.)),
                (4, px(16.)),
                (2, px(24.)),
                (2, px(28.)),
                (8, px(32.)),
            ],
            ends: vec![5],
            source_boundaries: vec![0, 2, 4, 6, 8],
        };
        let bounds = Bounds::new(gpui::Point::default(), size(px(60.), px(20.)));
        let range = 0..4;
        let rects = geometry.rects(bounds, TextAlign::Left, 0, std::slice::from_ref(&range));
        assert_eq!(rects.len(), 2);
        assert_eq!(
            rects[0],
            Bounds::new(point(px(0.), px(0.)), size(px(8.), px(20.)))
        );
        assert_eq!(
            rects[1],
            Bounds::new(point(px(24.), px(0.)), size(px(8.), px(20.)))
        );
    }
}
