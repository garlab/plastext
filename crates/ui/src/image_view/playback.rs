use plastext_platform::images::Info;
use std::time::{Duration, Instant};

pub(super) struct Playback {
    pub frame: u32,
    pub playing: bool,
    plays: u32,
    pub due: Instant,
    pub delay: Duration,
}
impl Default for Playback {
    fn default() -> Self {
        Self {
            frame: 0,
            playing: true,
            plays: 1,
            due: Instant::now(),
            delay: Duration::from_millis(100),
        }
    }
}
impl Playback {
    pub fn next(&self, info: &Info) -> Option<u32> {
        if !self.playing || info.frames < 2 {
            None
        } else if self.frame + 1 < info.frames {
            Some(self.frame + 1)
        } else if info.plays.is_none_or(|n| self.plays < n) {
            Some(0)
        } else {
            None
        }
    }
    pub fn present(&mut self, frame: u32, delay: u32) {
        if frame < self.frame {
            self.plays = self.plays.saturating_add(1);
        }
        self.frame = frame;
        self.delay = Duration::from_millis(delay as u64);
        self.due = Instant::now() + self.delay;
    }
    pub fn toggle(&mut self, info: &Info) {
        if self.playing && self.next(info).is_none() {
            self.frame = 0;
            self.plays = 1;
        } else {
            self.playing = !self.playing;
        }
        self.due = Instant::now() + self.delay;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_loops_end_at_the_last_frame_and_restart() {
        let info = Info {
            width: 1,
            height: 1,
            frames: 2,
            plays: Some(2),
            format: "GIF".into(),
            color: "RGB".into(),
        };
        let mut p = Playback::default();
        assert_eq!(p.next(&info), Some(1));
        p.present(1, 40);
        assert_eq!(p.next(&info), Some(0));
        p.present(0, 40);
        p.present(1, 40);
        assert_eq!(p.next(&info), None);
        p.toggle(&info);
        assert_eq!(p.frame, 0);
        assert_eq!(p.next(&info), Some(1));
        p.toggle(&info);
        assert_eq!(p.next(&info), None);
    }
}
