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
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    Pane(usize),
    Split(Axis, Vec<Child>),
}

/// A child's share of its parent. Weights need not add up to one.
#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub layout: Layout,
    pub weight: f32,
}

/// A divider's geometry when the mouse was pressed. Each divider is identified
/// by the first pane after it, which is unique throughout the tree.
#[derive(Clone, Copy)]
pub struct Resize {
    pub axis: Axis,
    pane_after: usize,
    before: f32,
    pair: f32,
    weight_per_pixel: f32,
    minimum_before: f32,
    minimum_after: f32,
}

impl Layout {
    /// Puts `new` after `pane`, splitting along `axis`. Returns whether `pane` was found.
    pub fn split(&mut self, pane: usize, axis: Axis, new: usize) -> bool {
        match self {
            Self::Pane(id) if *id == pane => {
                *self = Self::Split(
                    axis,
                    vec![
                        Child {
                            layout: Self::Pane(pane),
                            weight: 1.,
                        },
                        Child {
                            layout: Self::Pane(new),
                            weight: 1.,
                        },
                    ],
                );
                true
            }
            Self::Pane(_) => false,
            Self::Split(split_axis, children) => {
                if *split_axis == axis
                    && let Some(i) = children
                        .iter()
                        .position(|child| child.layout == Self::Pane(pane))
                {
                    let weight = children[i].weight / 2.;
                    children[i].weight = weight;
                    children.insert(
                        i + 1,
                        Child {
                            layout: Self::Pane(new),
                            weight,
                        },
                    );
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.layout.split(pane, axis, new))
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
            Self::Split(_, children) => children
                .iter()
                .flat_map(|child| child.layout.panes())
                .collect(),
        }
    }

    pub fn first_pane(&self) -> usize {
        match self {
            Self::Pane(id) => *id,
            Self::Split(_, children) => children[0].layout.first_pane(),
        }
    }

    /// Capture the adjacent pair, using the actual size of this split in pixels.
    pub fn begin_resize(&mut self, pane_after: usize, extent: f32) -> Option<Resize> {
        if !extent.is_finite() || extent <= 0. {
            return None;
        }
        let (axis, children, i) = self.divider(pane_after)?;
        let weight_per_pixel = children.iter().map(|child| child.weight).sum::<f32>() / extent;
        let before = children[i - 1].weight;
        let pair = before + children[i].weight;
        let minimum_before = children[i - 1].layout.minimum_extent(axis) * weight_per_pixel;
        let minimum_after = children[i].layout.minimum_extent(axis) * weight_per_pixel;
        // A small window can be narrower than the usual minimums. Scale both
        // minimums together so the divider still has a valid range.
        let shrink = (pair / (minimum_before + minimum_after)).min(1.);
        Some(Resize {
            axis,
            pane_after,
            before,
            pair,
            weight_per_pixel,
            minimum_before: minimum_before * shrink,
            minimum_after: minimum_after * shrink,
        })
    }

    /// Move only the two children touching the divider, preserving their total share.
    pub fn resize(&mut self, drag: Resize, delta: f32) {
        if !delta.is_finite() {
            return;
        }
        let Some((axis, children, i)) = self.divider(drag.pane_after) else {
            return;
        };
        if axis != drag.axis {
            return;
        }
        let maximum = (drag.pair - drag.minimum_after).max(drag.minimum_before);
        let before =
            (drag.before + delta * drag.weight_per_pixel).clamp(drag.minimum_before, maximum);
        children[i - 1].weight = before;
        children[i].weight = drag.pair - before;
    }

    fn divider(&mut self, pane_after: usize) -> Option<(Axis, &mut Vec<Child>, usize)> {
        let Self::Split(axis, children) = self else {
            return None;
        };
        if let Some(i) = children
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, child)| (child.layout.first_pane() == pane_after).then_some(i))
        {
            return Some((*axis, children, i));
        }
        children
            .iter_mut()
            .find_map(|child| child.layout.divider(pane_after))
    }

    fn minimum_extent(&self, axis: Axis) -> f32 {
        match self {
            Self::Pane(_) => match axis {
                Axis::Horizontal => 160.,
                Axis::Vertical => 100.,
            },
            Self::Split(split_axis, children) => {
                let sizes = children
                    .iter()
                    .map(|child| child.layout.minimum_extent(axis));
                if *split_axis == axis {
                    sizes.sum()
                } else {
                    sizes.fold(0., f32::max)
                }
            }
        }
    }

    fn remove_without_merging(&mut self, pane: usize) -> bool {
        let Self::Split(_, children) = self else {
            return false;
        };
        if let Some(i) = children
            .iter()
            .position(|child| child.layout == Self::Pane(pane))
        {
            children.remove(i);
            return true;
        }
        children
            .iter_mut()
            .any(|child| child.layout.remove_without_merging(pane))
    }

    /// Replaces one-child splits by their child and inlines same-axis child splits.
    fn merge(&mut self) {
        let Self::Split(axis, children) = self else {
            return;
        };
        let mut merged = Vec::with_capacity(children.len());
        for mut child in children.drain(..) {
            child.layout.merge();
            match child.layout {
                Self::Split(child_axis, grandchildren) if child_axis == *axis => {
                    let total: f32 = grandchildren.iter().map(|c| c.weight).sum();
                    merged.extend(grandchildren.into_iter().map(|mut grandchild| {
                        grandchild.weight *= child.weight / total;
                        grandchild
                    }));
                }
                layout => merged.push(Child {
                    layout,
                    weight: child.weight,
                }),
            }
        }
        *children = merged;
        if children.len() == 1 {
            *self = children.pop().expect("one child").layout;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, Axis::*, Child, Layout, Layout::*};

    fn group(axis: Axis, entries: Vec<(Layout, f32)>) -> Layout {
        Split(
            axis,
            entries
                .into_iter()
                .map(|(layout, weight)| Child { layout, weight })
                .collect(),
        )
    }

    #[test]
    fn splitting_along_the_same_axis_adds_siblings() {
        let mut layout = Pane(0);
        assert!(layout.split(0, Horizontal, 1));
        assert!(layout.split(0, Horizontal, 2));
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![(Pane(0), 0.5), (Pane(2), 0.5), (Pane(1), 1.)]
            )
        );
        assert!(layout.split(2, Vertical, 3));
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![
                    (Pane(0), 0.5),
                    (group(Vertical, vec![(Pane(2), 1.), (Pane(3), 1.)]), 0.5),
                    (Pane(1), 1.)
                ]
            )
        );
        assert_eq!(layout.panes(), [0, 2, 3, 1]);
        assert!(!layout.split(9, Vertical, 4));
    }

    #[test]
    fn removing_panes_merges_what_is_left() {
        let mut layout = group(
            Horizontal,
            vec![
                (Pane(0), 1.),
                (
                    group(
                        Vertical,
                        vec![
                            (Pane(1), 1.),
                            (group(Horizontal, vec![(Pane(2), 1.), (Pane(3), 3.)]), 1.),
                        ],
                    ),
                    2.,
                ),
            ],
        );
        assert!(layout.remove(1));
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![(Pane(0), 1.), (Pane(2), 0.5), (Pane(3), 1.5)]
            )
        );
        assert!(layout.remove(0));
        assert!(layout.remove(2));
        assert_eq!(layout, Pane(3));
        assert!(!layout.remove(3), "the last pane stays");
        assert!(!layout.remove(7));
    }

    #[test]
    fn dragging_changes_only_the_adjacent_pair_and_clamps_at_minimums() {
        let mut layout = group(
            Horizontal,
            vec![(Pane(0), 1.), (Pane(1), 1.), (Pane(2), 1.)],
        );
        let drag = layout.begin_resize(1, 1200.).unwrap();
        layout.resize(drag, 200.);
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![(Pane(0), 1.5), (Pane(1), 0.5), (Pane(2), 1.)]
            )
        );
        // Repeated moves use the initial grab position, without accumulating deltas.
        layout.resize(drag, 200.);
        layout.resize(drag, 0.);
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![(Pane(0), 1.), (Pane(1), 1.), (Pane(2), 1.)]
            )
        );
        layout.resize(drag, -5000.);
        let Split(_, children) = &layout else {
            unreachable!()
        };
        assert!((children[0].weight * 400. - 160.).abs() < 0.01);
        assert_eq!(children[2].weight, 1.);
        layout.resize(drag, 5000.);
        let Split(_, children) = &layout else {
            unreachable!()
        };
        assert!((children[1].weight * 400. - 160.).abs() < 0.01);
    }

    #[test]
    fn nested_vertical_dividers_and_new_splits_preserve_outer_shares() {
        let mut layout = group(
            Horizontal,
            vec![
                (Pane(0), 3.),
                (group(Vertical, vec![(Pane(1), 1.), (Pane(2), 1.)]), 1.),
            ],
        );
        let drag = layout.begin_resize(2, 800.).unwrap();
        assert_eq!(drag.axis, Vertical);
        layout.resize(drag, 200.);
        assert!(layout.split(1, Vertical, 3));
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![
                    (Pane(0), 3.),
                    (
                        group(
                            Vertical,
                            vec![(Pane(1), 0.75), (Pane(3), 0.75), (Pane(2), 0.5)]
                        ),
                        1.
                    ),
                ]
            )
        );
        assert!(layout.remove(3));
        assert_eq!(
            layout,
            group(
                Horizontal,
                vec![
                    (Pane(0), 3.),
                    (group(Vertical, vec![(Pane(1), 0.75), (Pane(2), 0.5)]), 1.),
                ]
            )
        );
        assert_eq!(layout.panes(), [0, 1, 2]);
    }

    #[test]
    fn small_or_unmeasured_splits_do_not_produce_invalid_weights() {
        let mut layout = group(Vertical, vec![(Pane(0), 1.), (Pane(1), 1.)]);
        assert!(layout.begin_resize(1, 0.).is_none());
        assert!(layout.begin_resize(1, f32::NAN).is_none());
        assert!(layout.begin_resize(9, 800.).is_none());
        let drag = layout.begin_resize(1, 120.).unwrap();
        layout.resize(drag, 1000.);
        assert_eq!(layout, group(Vertical, vec![(Pane(0), 1.), (Pane(1), 1.)]));
        layout.resize(drag, f32::NAN);
        assert_eq!(layout, group(Vertical, vec![(Pane(0), 1.), (Pane(1), 1.)]));
    }
}
