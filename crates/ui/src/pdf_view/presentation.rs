use plastext_platform::pdf::Tile;
use std::time::{Duration, Instant};

pub(super) const RESIZE_QUIET: Duration = Duration::from_millis(120);

/// Tile keys only: the global cache owns and charges every retained image.
/// Keep the displayed resolution until its replacement viewport is complete.
#[derive(Default)]
pub(super) struct Presentation {
    target: Option<(u32, u32)>,
    ready_at: Option<Instant>,
    displayed: Vec<Tile>,
}

impl Presentation {
    pub fn target(&mut self, page: u32, scale: u32, now: Instant) {
        let target = (page, scale);
        if self.target == Some(target) {
            return;
        }
        if self.target.is_some_and(|old| old.0 == page) {
            self.ready_at = Some(now + RESIZE_QUIET);
        } else {
            // A newly opened page starts immediately and never displays another page.
            self.ready_at = None;
            self.displayed.clear();
        }
        self.target = Some(target);
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.ready_at
            .and_then(|deadline| deadline.checked_duration_since(now))
            .filter(|delay| !delay.is_zero())
    }

    pub fn is_current(&self, tile: Tile) -> bool {
        self.target == Some((tile.page, tile.scale))
    }

    pub fn received(&mut self, tile: Tile) {
        // A resize during the very first render can still use that first tile.
        if self.displayed.is_empty() && self.target.is_some_and(|target| target.0 == tile.page) {
            self.displayed.push(tile);
        }
    }

    pub fn seed(&mut self, tiles: &[Tile]) {
        if self.displayed.is_empty()
            && !tiles.is_empty()
            && tiles
                .iter()
                .all(|tile| self.target.is_some_and(|target| target.0 == tile.page))
        {
            self.displayed.extend_from_slice(tiles);
        }
    }

    pub fn select(&mut self, wanted: &[Tile], available: &[Tile]) -> &[Tile] {
        let complete = !wanted.is_empty() && available == wanted;
        let same_resolution = self.displayed.first().is_some_and(|old| {
            wanted
                .first()
                .is_some_and(|new| (old.page, old.scale) == (new.page, new.scale))
        });
        // Initial loading and ordinary scrolling remain progressive. A resolution
        // change swaps the complete visible set, avoiding patches of differently
        // sized/antialiased text as individual replacement tiles arrive.
        if complete || (!available.is_empty() && (self.displayed.is_empty() || same_resolution)) {
            self.displayed.clear();
            self.displayed.extend_from_slice(available);
        }
        &self.displayed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plastext_platform::pdf::SCALE_UNIT;

    fn tiles(scale: u32) -> Vec<Tile> {
        (0..4)
            .map(|x| Tile {
                page: 1,
                scale,
                x: x * 512,
                y: 0,
            })
            .collect()
    }

    #[test]
    fn dragging_keeps_the_last_frame_and_only_the_latest_size_becomes_ready() {
        let start = Instant::now();
        let old = tiles(SCALE_UNIT);
        let mut presentation = Presentation::default();
        presentation.target(1, SCALE_UNIT, start);
        assert_eq!(presentation.remaining(start), None);
        assert_eq!(presentation.select(&old, &old), old);

        for i in 1..=100 {
            let now = start + Duration::from_millis(i * 8);
            let wanted = tiles(SCALE_UNIT + i as u32);
            presentation.target(1, wanted[0].scale, now);
            assert!(presentation.remaining(now).is_some());
            assert_eq!(presentation.select(&wanted, &[]), old);
        }
        let last_change = start + Duration::from_millis(800);
        // Unrelated paints do not extend the debounce.
        presentation.target(1, SCALE_UNIT + 100, last_change + Duration::from_millis(50));
        assert_eq!(presentation.remaining(last_change + RESIZE_QUIET), None);
        let wanted = tiles(SCALE_UNIT + 100);
        assert_eq!(presentation.select(&wanted, &wanted[..3]), old);
        assert_eq!(presentation.select(&wanted, &wanted), wanted);
    }

    #[test]
    fn reversing_zoom_reuses_cached_tiles_and_scrolling_does_not_wait() {
        let now = Instant::now();
        let old = tiles(SCALE_UNIT);
        let zoomed = tiles(SCALE_UNIT * 2);
        let mut presentation = Presentation::default();
        presentation.target(1, SCALE_UNIT, now);
        presentation.select(&old, &old);
        presentation.target(1, SCALE_UNIT * 2, now);
        assert_eq!(presentation.select(&zoomed, &zoomed[..1]), old);
        presentation.target(1, SCALE_UNIT, now);
        assert_eq!(presentation.select(&old, &old), old);
        presentation.target(1, SCALE_UNIT, now + RESIZE_QUIET);
        assert_eq!(presentation.remaining(now + RESIZE_QUIET), None);
        assert_eq!(presentation.select(&old[1..], &old[1..]), &old[1..]);
    }

    #[test]
    fn initial_tiles_are_progressive_and_page_changes_never_reuse_old_content() {
        let now = Instant::now();
        let old = tiles(SCALE_UNIT);
        let mut presentation = Presentation::default();
        presentation.target(1, SCALE_UNIT, now);
        presentation.received(old[0]);
        assert_eq!(presentation.select(&old, &old[..1]), &old[..1]);
        assert_eq!(presentation.select(&old, &old[..2]), &old[..2]);
        presentation.target(2, SCALE_UNIT, now);
        presentation.received(old[3]);
        assert!(presentation.select(&[], &[]).is_empty());
        assert!(!presentation.is_current(old[0]));
        assert_eq!(presentation.remaining(now), None);
    }
}
