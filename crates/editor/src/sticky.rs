//! Which rows sticky scroll pins above the text (spec CODE-7).

use plastext_syntax::Symbol;

/// The starts of the rows to pin: the first rows of the definitions that
/// enclose the top of the view, outermost first, at most `max` of them and
/// fewer than `visible.len()`.
///
/// Pinned rows cover the rows they're drawn over, so a definition whose first
/// row is among the covered ones counts too: the rows just below the pinned
/// ones decide. `visible` holds the start of each visible row, and
/// `row_start_at` finds the row containing an offset.
pub(crate) fn sticky_rows(
    symbols: &[Symbol],
    visible: &[u64],
    max: usize,
    row_start_at: impl Fn(u64) -> Option<u64>,
) -> Vec<u64> {
    if visible.len() < 2 {
        return Vec::new();
    }
    let max = max.min(visible.len() - 1);
    let enclosing = |probe: u64| -> Vec<u64> {
        let before = symbols.partition_point(|symbol| (symbol.range.start as u64) < probe);
        let mut starts: Vec<u64> = Vec::new();
        for symbol in &symbols[..before] {
            if (symbol.range.end as u64) <= probe {
                continue;
            }
            if let Some(start) = row_start_at(symbol.range.start as u64)
                && start < probe
                && starts.last() != Some(&start)
            {
                starts.push(start);
            }
        }
        let excess = starts.len().saturating_sub(max);
        starts.drain(..excess);
        starts
    };
    let mut starts = enclosing(visible[0]);
    for _ in 0..max {
        let below = visible[starts.len().min(visible.len() - 1)];
        let next = enclosing(below);
        if next == starts {
            break;
        }
        starts = next;
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows are 10 bytes long: row `n` starts at `10 * n`.
    fn row_start_at(offset: u64) -> Option<u64> {
        Some(offset / 10 * 10)
    }

    fn symbol(rows: std::ops::Range<u64>) -> Symbol {
        Symbol {
            name: String::new(),
            kind: "function".into(),
            range: (rows.start * 10 + 2) as usize..(rows.end * 10) as usize,
            depth: 0,
        }
    }

    fn visible_from(row: u64) -> Vec<u64> {
        (row..row + 20).map(|row| row * 10).collect()
    }

    #[test]
    fn pins_enclosing_definitions_outermost_first() {
        let symbols = [
            symbol(0..100),
            symbol(5..50),
            symbol(10..20),
            symbol(30..40),
        ];
        assert_eq!(
            sticky_rows(&symbols, &visible_from(12), 4, row_start_at),
            [0, 50, 100]
        );
        assert_eq!(
            sticky_rows(&symbols, &visible_from(32), 4, row_start_at),
            [0, 50, 300]
        );
        assert_eq!(
            sticky_rows(&symbols, &visible_from(60), 4, row_start_at),
            [0]
        );
        assert!(sticky_rows(&symbols, &visible_from(0), 4, row_start_at).is_empty());
    }

    #[test]
    fn counts_definitions_starting_under_the_pinned_rows() {
        // The view starts at row 1, which the pinned row 0 covers, so the
        // definition starting there is pinned too. One starting at row 2,
        // just below, stays visible in place.
        let symbols = [symbol(0..100), symbol(1..50)];
        assert_eq!(
            sticky_rows(&symbols, &visible_from(1), 4, row_start_at),
            [0, 10]
        );
        let symbols = [symbol(0..100), symbol(2..50)];
        assert_eq!(
            sticky_rows(&symbols, &visible_from(1), 4, row_start_at),
            [0]
        );
    }

    #[test]
    fn keeps_the_innermost_when_too_deep() {
        let symbols: Vec<Symbol> = (0..6).map(|depth| symbol(depth..100 - depth)).collect();
        assert_eq!(
            sticky_rows(&symbols, &visible_from(20), 2, row_start_at),
            [40, 50]
        );
        assert_eq!(
            sticky_rows(&symbols, &visible_from(20)[..2], 4, row_start_at).len(),
            1
        );
    }

    #[test]
    fn pins_a_row_once_for_definitions_on_the_same_line() {
        let mut inner = symbol(0..30);
        inner.range.start += 3;
        let symbols = [symbol(0..40), inner];
        assert_eq!(
            sticky_rows(&symbols, &visible_from(10), 4, row_start_at),
            [0]
        );
    }
}
