use gpui::{Bounds, Pixels, Point, point, px, size};
use plastext_platform::images::{Info, Region, SCALE_UNIT, TILE_EDGE};

pub(super) struct Viewport {
    pub pan: Point<f64>,
    pub origin: Point<f64>,
    pub zoom: f64,
    pub width: f64,
    pub height: f64,
    info: Info,
    scale: u32,
}
impl Viewport {
    pub fn overview(info: &Info, frame: u32, budget: usize) -> Region {
        let edge = ((budget as f64 / 32.).sqrt() as u32).clamp(1, 800);
        info.overview_at(frame, edge)
    }
    pub fn plan(&mut self, frame: u32, budget: usize) -> Vec<Region> {
        let overview = Self::overview(&self.info, frame, budget);
        let count = (budget.saturating_sub(overview.bytes() * 2) / (512 * 512 * 8))
            .min(
                (plastext_assets::imaging::MAX_BATCH_PIXELS as usize)
                    .saturating_sub(overview.bytes() / 4)
                    / (512 * 512),
            )
            .min(15);
        let mut plan = vec![overview];
        if count > 0 {
            self.fit_budget(count);
            let tiles = self.tiles(frame);
            if tiles.first().is_some_and(|t| t.scale > overview.scale) {
                plan.extend(tiles);
            }
        }
        plan
    }
    pub fn new(
        info: &Info,
        width: f64,
        height: f64,
        dpr: f64,
        zoom: Option<f64>,
        mut pan: Point<f64>,
    ) -> Self {
        let zoom = zoom
            .unwrap_or_else(|| {
                ((width - 32.).max(1.) / info.width as f64)
                    .min((height - 32.).max(1.) / info.height as f64)
                    .min(1.)
            })
            .clamp(1. / 100_000., 32.);
        let (w, h) = (info.width as f64 * zoom, info.height as f64 * zoom);
        pan.x = pan.x.clamp(0., (w + 32. - width).max(0.));
        pan.y = pan.y.clamp(0., (h + 32. - height).max(0.));
        let origin = point(
            ((width - w) / 2.).max(16.) - pan.x,
            ((height - h) / 2.).max(16.) - pan.y,
        );
        Self {
            pan,
            origin,
            zoom,
            width,
            height,
            info: info.clone(),
            scale: (zoom * dpr * SCALE_UNIT as f64)
                .floor()
                .clamp(1., (64 * SCALE_UNIT) as f64) as u32,
        }
    }
    pub fn fit_budget(&mut self, count: usize) {
        while self.tiles(0).len() > count.max(1) && self.scale > 1 {
            self.scale = (self.scale / 2).max(1);
        }
    }
    pub fn tiles(&self, frame: u32) -> Vec<Region> {
        let ratio = self.scale as f64 / SCALE_UNIT as f64 / self.zoom;
        let (w, h) = self.info.scaled_size(self.scale);
        let x0 = (-self.origin.x * ratio).max(0.) as u32 / TILE_EDGE * TILE_EDGE;
        let y0 = (-self.origin.y * ratio).max(0.) as u32 / TILE_EDGE * TILE_EDGE;
        let x1 = (((self.width - self.origin.x) * ratio).ceil().max(0.) as u32).min(w);
        let y1 = (((self.height - self.origin.y) * ratio).ceil().max(0.) as u32).min(h);
        let mut out = Vec::new();
        for y in (y0..y1).step_by(TILE_EDGE as usize) {
            for x in (x0..x1).step_by(TILE_EDGE as usize) {
                out.push(Region {
                    frame,
                    scale: self.scale,
                    x,
                    y,
                    width: TILE_EDGE.min(w - x),
                    height: TILE_EDGE.min(h - y),
                });
                if out.len() == 128 {
                    return out;
                }
            }
        }
        out
    }
    pub fn paper(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            origin + point(px(self.origin.x as f32), px(self.origin.y as f32)),
            size(
                px((self.info.width as f64 * self.zoom) as f32),
                px((self.info.height as f64 * self.zoom) as f32),
            ),
        )
    }
    pub fn rect(&self, region: Region, origin: Point<Pixels>) -> Bounds<Pixels> {
        let ratio = self.zoom * SCALE_UNIT as f64 / region.scale as f64;
        Bounds::new(
            origin
                + point(
                    px((self.origin.x + region.x as f64 * ratio) as f32),
                    px((self.origin.y + region.y as f64 * ratio) as f32),
                ),
            size(
                px((region.width as f64 * ratio) as f32),
                px((region.height as f64 * ratio) as f32),
            ),
        )
    }
    /// Keep the source pixel below the pointer fixed while zooming.
    pub fn zoom_pan(&self, zoom: f64, anchor: Point<f64>) -> Point<f64> {
        let new_origin = point(
            ((self.width - self.info.width as f64 * zoom) / 2.).max(16.),
            ((self.height - self.info.height as f64 * zoom) / 2.).max(16.),
        );
        point(
            new_origin.x - anchor.x + (anchor.x - self.origin.x) * zoom / self.zoom,
            new_origin.y - anchor.y + (anchor.y - self.origin.y) * zoom / self.zoom,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info(w: u32, h: u32) -> Info {
        Info {
            width: w,
            height: h,
            frames: 1,
            plays: Some(1),
            format: "PNG".into(),
            color: "RGB".into(),
        }
    }
    #[test]
    fn split_panes_keep_complete_frames_inside_their_cache_share() {
        for panes in [1, 2, 4, 16, 32, 128] {
            let budget = 64 * 1024 * 1024 / panes / 2;
            let info = info(40_000, 40_000);
            let mut view = Viewport::new(&info, 4000., 3000., 2., Some(1.), point(12000., 8000.));
            let plan = view.plan(0, budget);
            assert!(plan.iter().map(|r| r.bytes() * 2).sum::<usize>() <= budget);
            plastext_assets::imaging::validate_regions(&info, &plan).unwrap();
        }
    }
    #[test]
    fn fits_both_axes_without_upscaling() {
        for (w, h) in [(200, 100), (40_000, 40_000), (400, 4000)] {
            let v = Viewport::new(&info(w, h), 1000., 800., 2., None, point(0., 0.));
            assert!(v.zoom <= 1. && w as f64 * v.zoom <= 968. && h as f64 * v.zoom <= 768.);
            assert!((v.origin.x * 2. + w as f64 * v.zoom - 1000.).abs() < 0.001);
            assert!((v.origin.y * 2. + h as f64 * v.zoom - 800.).abs() < 0.001);
        }
    }
    #[test]
    fn tiles_are_bounded_and_budget_does_not_change_geometry() {
        let info = info(40_000, 40_000);
        let mut v = Viewport::new(&info, 1000., 800., 2., Some(1.), point(39_000., 39_200.));
        let paper = v.paper(point(px(0.), px(0.)));
        v.fit_budget(6);
        assert!(v.tiles(0).len() <= 6);
        assert_eq!(paper, v.paper(point(px(0.), px(0.))));
        for tile in v.tiles(0) {
            tile.validate(&info).unwrap();
        }
    }
    #[test]
    fn zoom_keeps_the_pointer_on_the_same_source_pixel() {
        let info = info(4000, 3000);
        let old = Viewport::new(&info, 1000., 800., 2., Some(1.), point(500., 600.));
        let anchor = point(200., 300.);
        let new = Viewport::new(&info, 1000., 800., 2., Some(3.), old.zoom_pan(3., anchor));
        assert!(
            ((anchor.x - old.origin.x) / old.zoom - (anchor.x - new.origin.x) / new.zoom).abs()
                < 1e-6
        );
        assert!(
            ((anchor.y - old.origin.y) / old.zoom - (anchor.y - new.origin.y) / new.zoom).abs()
                < 1e-6
        );
    }
}
