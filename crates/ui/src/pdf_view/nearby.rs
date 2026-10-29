use super::viewport::Viewport;
use gpui::point;
use plastext_platform::pdf::{PageInfo, Tile};
use std::collections::{BTreeMap, HashSet};

/// Metadata and tile keys only, limited to the current page and two either side.
#[derive(Default, Clone)]
pub(super) struct Nearby {
    page: u32,
    pages: u32,
    backwards: bool,
    info: BTreeMap<u32, PageInfo>,
    previews: BTreeMap<u32, Vec<Tile>>,
    failed: HashSet<u32>,
}
impl Nearby {
    pub fn visit(&mut self, page: u32, pages: u32) {
        if self.page != page {
            self.backwards = page < self.page;
        }
        self.page = page;
        self.pages = pages;
        let range = page.saturating_sub(2).max(1)..=page.saturating_add(2).min(pages);
        self.info.retain(|page, _| range.contains(page));
        self.previews.retain(|page, _| range.contains(page));
        self.failed.retain(|page| range.contains(page));
    }
    pub fn candidates(&self) -> Vec<u32> {
        let direction = if self.backwards { -1 } else { 1 };
        [direction, direction * 2, -direction, -direction * 2]
            .into_iter()
            .map(|offset| self.page as i64 + offset)
            .filter(|&page| page >= 1 && page <= self.pages as i64)
            .map(|page| page as u32)
            .filter(|page| !self.failed.contains(page))
            .collect()
    }
    pub fn info(&self, page: u32) -> Option<PageInfo> {
        self.info.get(&page).copied()
    }
    pub fn insert(&mut self, info: PageInfo) {
        if info.page.abs_diff(self.page) <= 2 {
            self.failed.remove(&info.page);
            self.info.insert(info.page, info);
        }
    }
    pub fn preview(&self, page: u32) -> &[Tile] {
        self.previews.get(&page).map_or(&[], Vec::as_slice)
    }
    pub fn ready(&mut self, page: u32, tiles: Vec<Tile>) {
        self.previews.insert(page, tiles);
    }
    pub fn failed(&mut self, page: u32) {
        self.failed.insert(page);
    }
    pub fn clear_previews(&mut self) {
        self.previews.clear();
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct PrefetchViewport {
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
    pub zoom: f64,
    pub fit_page: bool,
}
impl PrefetchViewport {
    /// Use the same raster scale as the visible page. Admit complete viewports
    /// in navigation order instead of shrinking them to fit a per-page quota.
    pub fn plan(self, pages: &[PageInfo], tile_budget: usize, mut bytes: usize) -> Vec<Vec<Tile>> {
        let mut plan = Vec::new();
        for &info in pages {
            let tiles = self.tiles(info, tile_budget);
            if tiles.is_empty() {
                continue;
            }
            let cost = tiles.iter().try_fold(0, |cost, tile| {
                let (width, height) = tile.dimensions(info).ok()?;
                Some(cost + width as usize * height as usize * 8)
            });
            if let Some(cost) = cost.filter(|&cost| cost <= bytes) {
                bytes -= cost;
                plan.push(tiles);
            }
        }
        plan
    }

    pub fn tiles(self, info: PageInfo, budget: usize) -> Vec<Tile> {
        let mut view = Viewport::new(
            info,
            self.width,
            self.height,
            self.dpr,
            self.zoom,
            self.fit_page,
            point(0., 0.),
        );
        if view.fit_budget(budget) {
            view.tiles(info.page)
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_view::presentation::Presentation;
    use std::time::Instant;
    fn info(page: u32) -> PageInfo {
        PageInfo {
            page,
            pages: 10_000,
            width: 600.,
            height: 800.,
        }
    }
    #[test]
    fn prefetch_follows_direction_and_never_scans_the_document() {
        let mut nearby = Nearby::default();
        nearby.visit(1, 10_000);
        assert_eq!(nearby.candidates(), [2, 3]);
        for page in 1..=100 {
            nearby.visit(page, 10_000);
            nearby.insert(info(page));
            for next in nearby.candidates() {
                nearby.insert(info(next));
            }
            assert!(nearby.info.len() <= 5);
            assert!(nearby.info(page + 1).is_some());
        }
        nearby.visit(99, 10_000);
        assert_eq!(nearby.candidates(), [98, 97, 100, 101]);
        nearby.failed(98);
        assert_eq!(nearby.candidates(), [97, 100, 101]);
        nearby.visit(10_000, 10_000);
        assert_eq!(nearby.candidates(), [9999, 9998]);
        assert!(nearby.info.is_empty());
    }
    #[test]
    fn preloaded_viewports_obey_budget_and_use_each_pages_dimensions() {
        let viewport = PrefetchViewport {
            width: 1500.,
            height: 1000.,
            dpr: 2.,
            zoom: 1.,
            fit_page: true,
        };
        for page in [
            info(2),
            PageInfo {
                page: 3,
                width: 1200.,
                height: 400.,
                ..info(3)
            },
        ] {
            let tiles = viewport.tiles(page, 4);
            assert!(!tiles.is_empty() && tiles.len() <= 4);
            for tile in tiles {
                assert!(tile.dimensions(page).is_ok());
            }
        }
        assert!(viewport.tiles(info(2), 0).is_empty());
    }

    #[test]
    fn page_turn_reuses_the_exact_display_tiles_without_a_quality_change() {
        let mut nearby = Nearby::default();
        nearby.visit(1, 10_000);
        nearby.insert(info(1));
        let next = PageInfo {
            width: 800.,
            height: 600.,
            ..info(2)
        };
        nearby.insert(next);
        let viewport = PrefetchViewport {
            width: 1500.,
            height: 1000.,
            dpr: 2.,
            zoom: 1.,
            fit_page: true,
        };
        let sharp = viewport.tiles(next, 32);
        let plan = viewport.plan(&[next], 32, 64 * 1024 * 1024);
        assert_eq!(plan.as_slice(), std::slice::from_ref(&sharp));
        nearby.ready(2, plan[0].clone());
        nearby.visit(2, 10_000);
        assert_eq!(nearby.info(2), Some(next));
        let mut presentation = Presentation::default();
        presentation.target(2, sharp[0].scale, Instant::now());
        // A differently scaled thumbnail must not sneak into a page turn.
        presentation.seed(&viewport.tiles(next, 4));
        assert!(presentation.select(&sharp, &[]).is_empty());
        presentation.seed(nearby.preview(2));
        assert_eq!(presentation.select(&sharp, &[]), sharp);
        assert_eq!(presentation.select(&sharp, &sharp), sharp);

        // A jump beyond the small cache never displays a previous page.
        nearby.visit(100, 10_000);
        presentation.target(100, sharp[0].scale, Instant::now());
        presentation.seed(&sharp);
        assert!(nearby.info(100).is_none());
        assert!(nearby.preview(2).is_empty());
        assert!(presentation.select(&[], &[]).is_empty());
    }

    #[test]
    fn preload_admits_whole_viewports_in_priority_order_within_spare_memory() {
        let pages = [info(2), info(3), info(1), info(4)];
        for (width, height, dpr, zoom, fit_page, tile_budget) in [
            (1500., 1000., 2., 1., true, 32),
            (2400., 1500., 2., 2., false, 32),
            (600., 700., 2., 1., true, 16),
            (600., 700., 1., 1., false, 4),
        ] {
            let viewport = PrefetchViewport {
                width,
                height,
                dpr,
                zoom,
                fit_page,
            };
            let expected = viewport.tiles(pages[0], tile_budget);
            let bytes: usize = expected
                .iter()
                .map(|tile| {
                    let (w, h) = tile.dimensions(pages[0]).unwrap();
                    w as usize * h as usize * 8
                })
                .sum();
            assert!(viewport.plan(&pages, tile_budget, bytes - 1).is_empty());
            let plan = viewport.plan(&pages, tile_budget, bytes * 2);
            assert_eq!(plan.len(), 2);
            assert_eq!(plan[0], expected);
            assert_eq!(plan[1], viewport.tiles(pages[1], tile_budget));
            // Direction reversal changes priority, without changing quality.
            let reverse = viewport.plan(&[pages[2], pages[0]], tile_budget, bytes);
            assert_eq!(reverse, [viewport.tiles(pages[2], tile_budget)]);
        }
    }
}
