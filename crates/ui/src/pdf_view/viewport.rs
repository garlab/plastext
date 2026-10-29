use gpui::{Bounds, Pixels, Point, point, px, size};
use plastext_platform::pdf::{PageInfo, SCALE_UNIT, TILE_SIZE, Tile};

pub(super) struct Viewport {
    pub pan: Point<f64>,
    origin: Point<f64>,
    width: f64,
    height: f64,
    page_width: f64,
    page_height: f64,
    device_scale: f64,
    scale: u32,
}
impl Viewport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        info: PageInfo,
        width: f64,
        height: f64,
        device_scale: f64,
        zoom: f64,
        fit_page: bool,
        mut pan: Point<f64>,
    ) -> Self {
        let fit_width = (width - 32.).max(1.) / info.width;
        let fit = if fit_page {
            fit_width.min((height - 32.).max(1.) / info.height)
        } else {
            fit_width
        };
        let unit = SCALE_UNIT as f64;
        let logical_scale = fit * zoom;
        let scale = (logical_scale * device_scale * unit)
            .floor()
            .clamp(1., 64. * unit) as u32;
        let device_scale = scale as f64 / unit / logical_scale;
        let page_width = info.width * logical_scale;
        let page_height = info.height * logical_scale;
        pan.x = pan.x.clamp(0., (page_width + 32. - width).max(0.));
        pan.y = pan.y.clamp(0., (page_height + 32. - height).max(0.));
        let origin = point(
            ((width - page_width) / 2.).max(16.) - pan.x,
            ((height - page_height) / 2.).max(16.) - pan.y,
        );
        Self {
            pan,
            origin,
            width,
            height,
            page_width,
            page_height,
            device_scale,
            scale,
        }
    }
    /// Reduce raster resolution, preserving page geometry and pan, until this
    /// viewport fits its share of the global cache. Never request an evict/reload loop.
    pub fn fit_budget(&mut self, max_tiles: usize) -> bool {
        if max_tiles == 0 {
            return false;
        }
        while self.tiles(1).len() > max_tiles {
            if self.scale <= 1 {
                return false;
            }
            let next = (self.scale / 2).max(1);
            self.device_scale *= next as f64 / self.scale as f64;
            self.scale = next;
        }
        true
    }
    pub fn tiles(&self, page: u32) -> Vec<Tile> {
        let first_x = (-self.origin.x * self.device_scale).max(0.) as u32 / TILE_SIZE;
        let first_y = (-self.origin.y * self.device_scale).max(0.) as u32 / TILE_SIZE;
        let last_x = (((self.width - self.origin.x).min(self.page_width) * self.device_scale)
            .ceil()
            .max(1.) as u32)
            .div_ceil(TILE_SIZE);
        let last_y = (((self.height - self.origin.y).min(self.page_height) * self.device_scale)
            .ceil()
            .max(1.) as u32)
            .div_ceil(TILE_SIZE);
        let mut tiles = Vec::new();
        for y in first_y..last_y {
            for x in first_x..last_x {
                // Never turn an anomalous viewport into an unbounded request list.
                if tiles.len() == 128 {
                    return tiles;
                }
                tiles.push(Tile {
                    page,
                    scale: self.scale,
                    x: x * TILE_SIZE,
                    y: y * TILE_SIZE,
                });
            }
        }
        tiles
    }
    pub fn scale(&self) -> u32 {
        self.scale
    }
    pub fn paper_bounds(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            origin + point(px(self.origin.x as f32), px(self.origin.y as f32)),
            size(px(self.page_width as f32), px(self.page_height as f32)),
        )
    }
    pub fn tile_bounds(
        &self,
        tile: Tile,
        width: f64,
        height: f64,
        origin: Point<Pixels>,
    ) -> Bounds<Pixels> {
        // Cached tiles can have an older raster scale during a resize. Their
        // coordinates are in that scale's pixels, not the current viewport's.
        let pixels_per_tile_pixel = self.scale as f64 / tile.scale as f64 / self.device_scale;
        Bounds::new(
            origin
                + point(
                    px((self.origin.x + tile.x as f64 * pixels_per_tile_pixel) as f32),
                    px((self.origin.y + tile.y as f64 * pixels_per_tile_pixel) as f32),
                ),
            size(
                px((width * pixels_per_tile_pixel) as f32),
                px((height * pixels_per_tile_pixel) as f32),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_page_only_requests_the_viewport_and_clamps_pan() {
        let info = PageInfo {
            pages: 10_000,
            page: 5000,
            width: 600.,
            height: 1_000_000.,
        };
        let view = Viewport::new(info, 1000., 800., 2., 2., false, point(1e12, 1e12));
        let tiles = view.tiles(info.page);
        assert!(tiles.len() <= 20);
        assert!(view.pan.y < 1e12);
        for tile in tiles {
            assert!(tile.dimensions(info).is_ok());
        }
    }
    #[test]
    fn fit_page_centers_and_uses_physical_pixel_tiles() {
        let info = PageInfo {
            pages: 1,
            page: 1,
            width: 600.,
            height: 800.,
        };
        let view = Viewport::new(info, 1000., 1000., 2., 1., true, point(0., 0.));
        assert!((view.origin.x * 2. + view.page_width - 1000.).abs() < 0.01);
        assert!((view.origin.y * 2. + view.page_height - 1000.).abs() < 0.01);
        assert_eq!(view.pan, point(0., 0.));
        assert!(view.tiles(1).len() > 4);
    }

    #[test]
    fn cache_pressure_reduces_resolution_without_changing_page_geometry() {
        let info = PageInfo {
            pages: 1,
            page: 1,
            width: 600.,
            height: 800.,
        };
        let mut view = Viewport::new(info, 4000., 3000., 2., 2., false, point(500., 700.));
        let paper = view.paper_bounds(point(px(0.), px(0.)));
        assert!(view.fit_budget(8));
        assert!(view.tiles(1).len() <= 8);
        assert_eq!(view.paper_bounds(point(px(0.), px(0.))), paper);
        assert!(!view.fit_budget(0));
    }

    #[test]
    fn old_tiles_follow_page_geometry_through_resize_zoom_and_retina_changes() {
        let info = PageInfo {
            pages: 1,
            page: 1,
            width: 600.,
            height: 800.,
        };
        let old = Viewport::new(info, 1000., 800., 2., 1., false, point(0., 0.));
        let tile = Tile {
            page: 1,
            scale: old.scale(),
            x: 512,
            y: 512,
        };
        for (width, height, dpr, zoom, fit) in [
            (500., 800., 2., 1., false),
            (1500., 800., 2., 1., false),
            (1000., 300., 2., 1., true),
            (1000., 800., 1., 1.5, false),
        ] {
            let view = Viewport::new(info, width, height, dpr, zoom, fit, point(20., 100.));
            let origin = point(px(70.), px(40.));
            let paper = view.paper_bounds(origin);
            let bounds = view.tile_bounds(tile, 512., 512., origin);
            let fraction = 512. * SCALE_UNIT as f64 / tile.scale as f64 / info.width;
            let normalized_x = (bounds.left() - paper.left()) / paper.size.width;
            let normalized_width = bounds.size.width / paper.size.width;
            assert!((normalized_x as f64 - fraction).abs() < 1e-6);
            assert!((normalized_width as f64 - fraction).abs() < 1e-6);
            let adjacent = view.tile_bounds(Tile { x: 1024, ..tile }, 512., 512., origin);
            assert!(f32::from(bounds.right() - adjacent.left()).abs() < 0.001);
        }
    }
}
