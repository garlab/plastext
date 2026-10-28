/// Discrete document sizes shared by every tab in a window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Zoom(usize);

const LEVELS: &[u16] = &[50, 67, 80, 90, 100, 110, 125, 150, 175, 200];

impl Default for Zoom {
    fn default() -> Self {
        Self(4)
    }
}

impl Zoom {
    pub fn step(self, delta: isize) -> Self {
        Self(self.0.saturating_add_signed(delta).min(LEVELS.len() - 1))
    }

    pub fn percent(self) -> u16 {
        LEVELS[self.0]
    }

    pub fn factor(self) -> f32 {
        self.percent() as f32 / 100.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_are_reversible_and_bounded() {
        let normal = Zoom::default();
        assert_eq!(normal.percent(), 100);
        assert_eq!(normal.step(1).percent(), 110);
        assert_eq!(normal.step(1).step(-1).percent(), 100);
        assert_eq!(normal.step(-100).step(-1).percent(), 50);
        assert_eq!(normal.step(100).step(1).percent(), 200);
    }
}
