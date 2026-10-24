//! Smooth scrolling over rows that are found one at a time.

use gpui::{Pixels, px};

/// A scroll position: the row at the top, and how much of it is scrolled
/// out of view, in `0..line_height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollPosition<Row> {
    pub top: Row,
    pub offset: Pixels,
}

/// Scrolls `position` down by `delta` (up if negative), a row at a time.
///
/// `next` and `previous` give the neighboring rows, `None` past either end.
/// Scrolling stops with the first row fully shown at the top, or the last
/// row fully shown at the top: the last row is never partly scrolled away.
/// An error stops scrolling where it happened and is returned.
pub(crate) fn scroll<Row: Copy, E>(
    position: ScrollPosition<Row>,
    delta: Pixels,
    line_height: Pixels,
    mut next: impl FnMut(Row) -> Result<Option<Row>, E>,
    mut previous: impl FnMut(Row) -> Result<Option<Row>, E>,
) -> (ScrollPosition<Row>, Option<E>) {
    let ScrollPosition { mut top, offset } = position;
    if line_height <= px(0.) {
        return (position, None);
    }
    let stop = |top, err| {
        (
            ScrollPosition {
                top,
                offset: px(0.),
            },
            err,
        )
    };
    let mut offset = offset + delta;
    while offset >= line_height {
        match next(top) {
            Ok(Some(row)) => {
                top = row;
                offset -= line_height;
            }
            Ok(None) => return stop(top, None),
            Err(err) => return stop(top, Some(err)),
        }
    }
    while offset < px(0.) {
        match previous(top) {
            Ok(Some(row)) => {
                top = row;
                offset += line_height;
            }
            Ok(None) => return stop(top, None),
            Err(err) => return stop(top, Some(err)),
        }
    }
    if offset > px(0.) {
        match next(top) {
            Ok(Some(_)) => {}
            Ok(None) => return stop(top, None),
            Err(err) => return (ScrollPosition { top, offset }, Some(err)),
        }
    }
    (ScrollPosition { top, offset }, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: Pixels = px(20.);
    const LAST: u32 = 9;

    fn scroll_rows(top: u32, offset: f32, delta: f32) -> ScrollPosition<u32> {
        let next = |row: u32| Ok::<_, ()>((row < LAST).then_some(row + 1));
        let previous = |row: u32| Ok::<_, ()>(row.checked_sub(1));
        let position = ScrollPosition {
            top,
            offset: px(offset),
        };
        let (position, err) = scroll(position, px(delta), LINE, next, previous);
        assert_eq!(err, None);
        position
    }

    fn at(top: u32, offset: f32) -> ScrollPosition<u32> {
        ScrollPosition {
            top,
            offset: px(offset),
        }
    }

    #[test]
    fn scrolls_by_rows_and_fractions() {
        assert_eq!(scroll_rows(0, 0., 25.), at(1, 5.));
        assert_eq!(scroll_rows(1, 5., -10.), at(0, 15.));
        assert_eq!(scroll_rows(3, 0., 60.), at(6, 0.));
    }

    #[test]
    fn stops_at_the_first_row() {
        assert_eq!(scroll_rows(0, 0., -5.), at(0, 0.));
        assert_eq!(scroll_rows(2, 10., -100.), at(0, 0.));
    }

    #[test]
    fn never_scrolls_the_last_row_partly_away() {
        // Small steps at the end used to leave the last row half scrolled,
        // then snap it back on the next step.
        let mut position = at(LAST, 0.);
        for _ in 0..10 {
            position = scroll_rows(position.top, f32::from(position.offset), 7.);
            assert_eq!(position, at(LAST, 0.));
        }
        assert_eq!(scroll_rows(LAST - 1, 15., 10.), at(LAST, 0.));
        assert_eq!(scroll_rows(LAST - 1, 0., 1000.), at(LAST, 0.));
    }

    #[test]
    fn errors_stop_scrolling() {
        let next = |row: u32| {
            if row < 2 {
                Ok(Some(row + 1))
            } else {
                Err("not loaded")
            }
        };
        let previous = |row: u32| Ok(row.checked_sub(1));
        let (position, err) = scroll(at(0, 0.), px(100.), LINE, next, previous);
        assert_eq!(position, at(2, 0.));
        assert_eq!(err, Some("not loaded"));
    }
}
