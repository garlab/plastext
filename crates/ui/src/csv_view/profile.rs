//! Opt-in table benchmark. Normal views never schedule these frame callbacks.
use super::*;
use std::{ops::Range, time::Instant};

pub(super) struct ReadingProfile {
    opened: Instant,
    painted: bool,
    preview_painted: bool,
    complete_painted: bool,
    frames_left: usize,
    steps: usize,
    pending: bool,
    previous_frame: Option<Instant>,
    targets: Vec<usize>,
    visited: HashSet<usize>,
    seen_pages: HashSet<u64>,
    page_loads: usize,
    page_reloads: usize,
    placeholders: usize,
    ready: bool,
    previous_top: Option<usize>,
    scrolled_frames: usize,
    rendered: Range<usize>,
}

impl ReadingProfile {
    pub(super) fn loaded_page(&mut self, page: u64) {
        self.page_loads += 1;
        if !self.seen_pages.insert(page) {
            self.page_reloads += 1;
        }
    }

    pub(super) fn rendered_rows(&mut self, range: Range<usize>, pages: &HashMap<u64, Vec<CsvRow>>) {
        self.ready = !range.is_empty()
            && range.clone().all(|row| {
                let record = row as u64 + 1;
                pages
                    .get(&(record / PAGE))
                    .and_then(|rows| rows.get((record % PAGE) as usize))
                    .is_some_and(|row| row.cells.is_ok())
            });
        // UniformList also calls its renderer for a sizing probe. Only the
        // last range presented by the paint callback counts as visible work.
        self.rendered = range;
    }
}

impl CsvView {
    pub fn profile_reading(&mut self, opened: Instant, frames: usize) {
        self.profile = Some(ReadingProfile {
            opened,
            painted: false,
            preview_painted: false,
            complete_painted: false,
            frames_left: frames.max(1),
            steps: 0,
            pending: false,
            previous_frame: None,
            targets: Vec::new(),
            visited: HashSet::new(),
            seen_pages: HashSet::new(),
            page_loads: 0,
            page_reloads: 0,
            placeholders: 0,
            ready: false,
            previous_top: None,
            scrolled_frames: 0,
            rendered: 0..0,
        });
    }

    pub(super) fn profile_painted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = &mut self.profile else {
            return;
        };
        if self.show_raw || self.notice.is_some() {
            eprintln!("reading: aborted: full table preview is unavailable");
            cx.quit();
            return;
        }
        if profile.pending {
            return;
        }
        let progress = self.index.progress();
        let ready = profile.ready;
        let first = !profile.painted;
        let first_preview = ready && !profile.preview_painted;
        let complete = ready && progress.complete;
        let first_complete = complete && !profile.complete_painted;
        if first_complete {
            let last = progress.rows.saturating_sub(2) as usize;
            profile.targets = vec![0, last / 2, last];
            profile.targets.sort_unstable();
            profile.targets.dedup();
            profile.steps = 0;
            eprintln!("reading: csv preview");
        }
        if ready {
            if profile
                .previous_top
                .replace(profile.rendered.start)
                .is_some_and(|top| top != profile.rendered.start)
            {
                profile.scrolled_frames += 1;
            }
            for &target in &profile.targets {
                if profile.rendered.contains(&target) {
                    profile.visited.insert(target);
                }
            }
            let now = Instant::now();
            if let Some(previous) = profile.previous_frame.replace(now) {
                eprintln!(
                    "reading: frame interval {:.6} ms",
                    now.duration_since(previous).as_secs_f64() * 1000.
                );
            }
            if !first_preview {
                profile.frames_left = profile.frames_left.saturating_sub(1);
            }
        } else if progress.complete {
            profile.placeholders += 1;
        }
        profile.painted = true;
        profile.preview_painted |= ready;
        profile.complete_painted |= complete;
        if !first && !ready {
            return;
        }
        let done =
            complete && profile.frames_left == 0 && profile.visited.len() == profile.targets.len();
        if done {
            eprintln!("reading: cached pages {}", self.pages.len());
            eprintln!(
                "reading: cached rows {}",
                self.pages.values().map(Vec::len).sum::<usize>()
            );
            eprintln!("reading: placeholder frames {}", profile.placeholders);
            eprintln!("reading: visited targets {}", profile.visited.len());
            eprintln!("reading: required targets {}", profile.targets.len());
            eprintln!("reading: page loads {}", profile.page_loads);
            eprintln!("reading: page reloads {}", profile.page_reloads);
            eprintln!("reading: scrolled frames {}", profile.scrolled_frames);
        }
        let opened = profile.opened;
        profile.pending = true;
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            if first {
                eprintln!(
                    "reading: first paint {:.3} ms",
                    opened.elapsed().as_secs_f64() * 1000.
                );
            }
            if first_preview {
                eprintln!(
                    "reading: preview paint {:.3} ms",
                    opened.elapsed().as_secs_f64() * 1000.
                );
                eprintln!("reading: scrolling");
            }
            if first_complete {
                eprintln!(
                    "reading: full preview paint {:.3} ms",
                    opened.elapsed().as_secs_f64() * 1000.
                );
            }
            if done {
                eprintln!("reading: done");
                cx.quit();
                return;
            }
            view.update(cx, |this, cx| {
                let Some(profile) = &mut this.profile else {
                    return;
                };
                profile.pending = false;
                if ready {
                    profile.steps += 1;
                    let count = this.index.progress().rows.saturating_sub(1) as usize;
                    let row = if complete && profile.steps.is_multiple_of(250) {
                        let jump = profile.steps / 250;
                        if jump % 2 == 1 {
                            profile.targets[(jump / 2) % profile.targets.len()]
                        } else {
                            jump.wrapping_mul(104_729) % count.max(1)
                        }
                    } else {
                        (this.first_row as usize + 3).min(count.saturating_sub(1))
                    };
                    this.scroll.scroll_to_item_strict(row, ScrollStrategy::Top);
                }
                cx.notify();
            })
            .ok();
        });
    }
}
