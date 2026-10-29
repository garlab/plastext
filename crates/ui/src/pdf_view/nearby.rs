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
    fn page_turn_uses_cached_dimensions_and_preview_until_sharp_tiles_are_ready() {
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
        let coarse = viewport.tiles(next, 4);
        nearby.ready(2, coarse.clone());
        nearby.visit(2, 10_000);
        assert_eq!(nearby.info(2), Some(next));
        let sharp = viewport.tiles(next, 32);
        assert!(sharp[0].scale > coarse[0].scale);
        let mut presentation = Presentation::default();
        presentation.target(2, sharp[0].scale, Instant::now());
        presentation.seed(nearby.preview(2));
        assert_eq!(presentation.select(&sharp, &[]), coarse);
        assert_eq!(presentation.select(&sharp, &sharp[..1]), coarse);
        assert_eq!(presentation.select(&sharp, &sharp), sharp);

        // A jump beyond the small cache never displays a previous page.
        nearby.visit(100, 10_000);
        presentation.target(100, sharp[0].scale, Instant::now());
        presentation.seed(&coarse);
        assert!(nearby.info(100).is_none());
        assert!(nearby.preview(2).is_empty());
        assert!(presentation.select(&[], &[]).is_empty());
    }
}
