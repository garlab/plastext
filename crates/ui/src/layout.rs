//! How the editor area arranges its panes: nested horizontal and vertical splits.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Panes side by side.
    Horizontal,
    /// Panes stacked.
    Vertical,
}

/// A tree of panes, identified by number. Splits always have at least two
/// children, and never a child split along the same axis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Pane(usize),
    Split(Axis, Vec<Layout>),
}

impl Layout {
    /// Puts `new` after `pane`, splitting along `axis`. Returns whether `pane` was found.
    pub fn split(&mut self, pane: usize, axis: Axis, new: usize) -> bool {
        match self {
            Self::Pane(id) if *id == pane => {
                *self = Self::Split(axis, vec![Self::Pane(pane), Self::Pane(new)]);
                true
            }
            Self::Pane(_) => false,
            Self::Split(split_axis, children) => {
                if *split_axis == axis
                    && let Some(i) = children.iter().position(|child| *child == Self::Pane(pane))
                {
                    children.insert(i + 1, Self::Pane(new));
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.split(pane, axis, new))
            }
        }
    }

    /// Removes `pane`, merging what's left. The last pane can't be removed.
    pub fn remove(&mut self, pane: usize) -> bool {
        if !self.remove_without_merging(pane) {
            return false;
        }
        self.merge();
        true
    }

    /// Panes in reading order.
    pub fn panes(&self) -> Vec<usize> {
        match self {
            Self::Pane(id) => vec![*id],
            Self::Split(_, children) => children.iter().flat_map(Self::panes).collect(),
        }
    }

    fn remove_without_merging(&mut self, pane: usize) -> bool {
        let Self::Split(_, children) = self else {
            return false;
        };
        if let Some(i) = children.iter().position(|child| *child == Self::Pane(pane)) {
            children.remove(i);
            return true;
        }
        children
            .iter_mut()
            .any(|child| child.remove_without_merging(pane))
    }

    /// Replaces one-child splits by their child and inlines same-axis child splits.
    fn merge(&mut self) {
        let Self::Split(axis, children) = self else {
            return;
        };
        let mut merged = Vec::with_capacity(children.len());
        for mut child in children.drain(..) {
            child.merge();
            match child {
                Self::Split(child_axis, grandchildren) if child_axis == *axis => {
                    merged.extend(grandchildren)
                }
                child => merged.push(child),
            }
        }
        *children = merged;
        if children.len() == 1 {
            *self = children.pop().expect("one child");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis::*, Layout::*};

    #[test]
    fn splitting_along_the_same_axis_adds_siblings() {
        let mut layout = Pane(0);
        assert!(layout.split(0, Horizontal, 1));
        assert!(layout.split(0, Horizontal, 2));
        assert_eq!(layout, Split(Horizontal, vec![Pane(0), Pane(2), Pane(1)]));
        assert!(layout.split(2, Vertical, 3));
        assert_eq!(
            layout,
            Split(
                Horizontal,
                vec![Pane(0), Split(Vertical, vec![Pane(2), Pane(3)]), Pane(1)]
            )
        );
        assert_eq!(layout.panes(), [0, 2, 3, 1]);
        assert!(!layout.split(9, Vertical, 4));
    }

    #[test]
    fn removing_panes_merges_what_is_left() {
        let mut layout = Split(
            Horizontal,
            vec![
                Pane(0),
                Split(
                    Vertical,
                    vec![Pane(1), Split(Horizontal, vec![Pane(2), Pane(3)])],
                ),
            ],
        );
        assert!(layout.remove(1));
        assert_eq!(layout, Split(Horizontal, vec![Pane(0), Pane(2), Pane(3)]));
        assert!(layout.remove(0));
        assert!(layout.remove(2));
        assert_eq!(layout, Pane(3));
        assert!(!layout.remove(3), "the last pane stays");
        assert!(!layout.remove(7));
    }
}
