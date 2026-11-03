use gpui::{Bounds, Pixels, Size, point, px, size};

/// Fit the whole diagram, including coordinates, inside the measured viewport.
/// The existing raster is reused during resizing; fitting needs no render job.
pub(super) fn fit(viewport: Bounds<Pixels>, image: Size<Pixels>) -> Option<Bounds<Pixels>> {
    let width = f32::from(viewport.size.width);
    let height = f32::from(viewport.size.height);
    let image_width = f32::from(image.width);
    let image_height = f32::from(image.height);
    if width <= 0. || height <= 0. || image_width <= 0. || image_height <= 0. {
        return None;
    }
    let margin = 12_f32.min(width * 0.1).min(height * 0.1);
    let scale = ((width - margin * 2.) / image_width).min((height - margin * 2.) / image_height);
    let fitted = size(px(image_width * scale), px(image_height * scale));
    Some(Bounds {
        origin: viewport.origin
            + point(
                (viewport.size.width - fitted.width) / 2.,
                (viewport.size.height - fitted.height) / 2.,
            ),
        size: fitted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_pane_fits_board_to_height_and_centers_it() {
        let pane = Bounds::new(point(px(710.), px(150.)), size(px(1320.), px(880.)));
        let board = fit(pane, size(px(1000.), px(1000.))).unwrap();
        assert_eq!(board.origin, point(px(942.), px(162.)));
        assert_eq!(board.size, size(px(856.), px(856.)));
    }

    #[test]
    fn resizing_keeps_square_and_rectangular_boards_inside_the_pane() {
        for (width, height) in [(300., 700.), (900., 120.), (20., 10.), (1., 1.)] {
            let pane = Bounds::new(point(px(120.), px(50.)), size(px(width), px(height)));
            for (iw, ih) in [(1000., 1000.), (504., 360.), (360., 504.)] {
                let board = fit(pane, size(px(iw), px(ih))).unwrap();
                assert!(board.left() >= pane.left() && board.right() <= pane.right());
                assert!(board.top() >= pane.top() && board.bottom() <= pane.bottom());
                assert!(
                    (f32::from(board.size.width) / f32::from(board.size.height) - iw / ih).abs()
                        < 0.001
                );
                assert_eq!(board.center(), pane.center());
            }
        }
        assert!(fit(Bounds::default(), size(px(1000.), px(1000.))).is_none());
    }
}
