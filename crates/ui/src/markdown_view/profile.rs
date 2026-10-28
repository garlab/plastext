//! Opt-in native benchmark; ordinary views do not schedule frame callbacks.
use super::*;
use std::time::Instant;

pub(super) struct ReadingProfile {
    opened: Instant,
    painted: bool,
    preview_painted: bool,
    complete_painted: bool,
    frames_left: usize,
    steps: usize,
    frame_pending: bool,
    previous_frame: Option<Instant>,
    targets: Vec<usize>,
    visited: HashSet<usize>,
    placeholder_frames: usize,
    page_loads: usize,
    page_reloads: usize,
    seen_pages: HashSet<usize>,
}

impl ReadingProfile {
    pub(super) fn loaded_page(&mut self, page: usize) {
        self.page_loads += 1;
        if !self.seen_pages.insert(page) {
            self.page_reloads += 1;
        }
    }
    pub(super) fn rendered_target(&mut self, row: usize) {
        if self.targets.contains(&row) {
            self.visited.insert(row);
        }
    }
}

impl MarkdownView {
    pub fn profile_reading(&mut self, opened: Instant, frames: usize) {
        self.profile = Some(ReadingProfile {
            opened,
            painted: false,
            preview_painted: false,
            complete_painted: false,
            frames_left: frames.max(1),
            steps: 0,
            frame_pending: false,
            previous_frame: None,
            targets: Vec::new(),
            visited: HashSet::new(),
            placeholder_frames: 0,
            page_loads: 0,
            page_reloads: 0,
            seen_pages: HashSet::from([0]),
        });
    }

    pub(super) fn profile_painted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile.is_some()
            && (!self.rendered
                || (!self.parsing && !self.preview_complete)
                || !self.failed_pages.is_empty())
        {
            eprintln!("reading: aborted: full rendered preview is unavailable");
            cx.quit();
            return;
        }
        let Some(profile) = &mut self.profile else {
            return;
        };
        if profile.frame_pending {
            return;
        }
        let top = self.list.logical_scroll_top().item_ix;
        let ready = self
            .visible
            .block(top)
            .and_then(|ix| self.document.blocks.get(ix))
            .is_some_and(|entry| self.pages.contains(entry.page as usize));
        if !ready && self.preview_complete {
            profile.placeholder_frames += 1;
        }
        if ready && self.preview_complete && profile.targets.contains(&top) {
            profile.visited.insert(top);
        }
        let complete = ready && self.preview_complete;
        let first = !profile.painted;
        let first_preview = ready && !profile.preview_painted;
        let first_complete = complete && !profile.complete_painted;
        if first_complete {
            let last = self.document.blocks.len().saturating_sub(1);
            let code = self
                .document
                .blocks
                .iter()
                .position(|b| matches!(b.kind, plastext_markdown::BlockType::Code));
            let prose = self
                .document
                .blocks
                .iter()
                .position(|b| matches!(b.kind, plastext_markdown::BlockType::Paragraph));
            profile.targets = vec![
                0,
                code.unwrap_or(last / 4),
                prose.unwrap_or(last / 2),
                last / 2,
                last,
            ]
            .into_iter()
            .map(|block| self.visible.row(block))
            .collect();
            profile.steps = 0;
            if profile.targets.contains(&top) {
                profile.visited.insert(top);
            }
            eprintln!("reading: indexed preview");
        }
        if ready {
            let now = Instant::now();
            if let Some(previous) = profile.previous_frame.replace(now) {
                eprintln!(
                    "reading: frame interval {:.6} ms",
                    now.duration_since(previous).as_secs_f64() * 1000.
                );
            }
        }
        profile.painted = true;
        profile.preview_painted |= ready;
        profile.complete_painted |= complete;
        if ready && !first_preview {
            profile.frames_left = profile.frames_left.saturating_sub(1);
        }
        if !first && !ready {
            return;
        }
        let required = profile
            .targets
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len();
        let done = complete && profile.frames_left == 0 && profile.visited.len() == required;
        if done {
            eprintln!("reading: index bytes {}", self.document.retained_bytes());
            eprintln!("reading: parsed cache bytes {}", self.pages.bytes());
            eprintln!("reading: placeholder frames {}", profile.placeholder_frames);
            eprintln!("reading: visited targets {}", profile.visited.len());
            eprintln!("reading: required targets {required}");
            eprintln!("reading: page loads {}", profile.page_loads);
            eprintln!("reading: page reloads {}", profile.page_reloads);
        }
        let opened = profile.opened;
        profile.frame_pending = true;
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
                profile.frame_pending = false;
                if ready {
                    profile.steps += 1;
                    // Exercise distant sections after parsing as well as ordinary
                    // sequential scroll. This reaches the table, fence and prose
                    // in the 100 MB fixture instead of staying in its first rows.
                    if complete && profile.steps.is_multiple_of(250) {
                        let jump = profile.steps / 250;
                        let item_ix = if jump % 2 == 1 {
                            profile.targets[(jump / 2) % profile.targets.len()]
                        } else {
                            // Distant jumps exercise cold pages and evict old
                            // prose pages before the fixed targets recur.
                            let block = jump.wrapping_mul(104_729) % this.document.blocks.len();
                            this.visible.row(block)
                        };
                        this.list.scroll_to(ListOffset {
                            item_ix,
                            offset_in_item: px(0.),
                        });
                    } else {
                        this.list.scroll_by(px(96.));
                    }
                    cx.notify();
                } else if !this.document.blocks.is_empty() {
                    // A short document can finish between painting the loading
                    // state and this callback. Ensure its first content is drawn.
                    cx.notify();
                }
            })
            .ok();
        });
    }
}
