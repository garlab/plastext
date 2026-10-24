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
        });
    }

    pub(super) fn profile_painted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile.is_some() && (!self.rendered || (!self.parsing && !self.preview_complete)) {
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
        let ready = !self.document.blocks.is_empty();
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
                .position(|b| matches!(b.kind, BlockKind::Code { .. }));
            let prose = self
                .document
                .blocks
                .iter()
                .position(|b| matches!(b.kind, BlockKind::Paragraph));
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
        let done = complete && profile.frames_left == 0;
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
                        let target = (profile.steps / 250) % profile.targets.len();
                        let item_ix = profile.targets[target];
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
