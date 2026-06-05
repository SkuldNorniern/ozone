use crate::view::ViewId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PaneTree {
    Leaf {
        view_id: ViewId,
    },
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

impl PaneTree {
    pub fn leaf(view_id: ViewId) -> Self {
        Self::Leaf { view_id }
    }

    pub fn split_leaf(
        &mut self,
        target: ViewId,
        new_view: ViewId,
        axis: SplitAxis,
        ratio: f32,
    ) -> bool {
        match self {
            Self::Leaf { view_id } if *view_id == target => {
                let old_view = *view_id;
                *self = Self::Split {
                    axis,
                    ratio: clamp_ratio(ratio),
                    first: Box::new(Self::Leaf { view_id: old_view }),
                    second: Box::new(Self::Leaf { view_id: new_view }),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, new_view, axis, ratio)
                    || second.split_leaf(target, new_view, axis, ratio)
            }
        }
    }

    pub fn contains(&self, target: ViewId) -> bool {
        match self {
            Self::Leaf { view_id } => *view_id == target,
            Self::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }

    pub fn leaves(&self) -> Vec<ViewId> {
        let mut out = Vec::new();
        self.push_leaves(&mut out);
        out
    }

    pub fn first_leaf(&self) -> ViewId {
        match self {
            Self::Leaf { view_id } => *view_id,
            Self::Split { first, .. } => first.first_leaf(),
        }
    }

    pub fn remove_leaf(&mut self, target: ViewId) -> Option<ViewId> {
        match self {
            Self::Leaf { view_id } => {
                if *view_id == target {
                    Some(*view_id)
                } else {
                    None
                }
            }
            Self::Split { first, second, .. } => {
                if first.contains(target) {
                    let removed = first.remove_leaf(target)?;
                    if matches!(**first, Self::Leaf { view_id } if view_id == removed) {
                        *self = (**second).clone();
                    }
                    Some(removed)
                } else if second.contains(target) {
                    let removed = second.remove_leaf(target)?;
                    if matches!(**second, Self::Leaf { view_id } if view_id == removed) {
                        *self = (**first).clone();
                    }
                    Some(removed)
                } else {
                    None
                }
            }
        }
    }

    fn push_leaves(&self, out: &mut Vec<ViewId>) {
        match self {
            Self::Leaf { view_id } => out.push(*view_id),
            Self::Split { first, second, .. } => {
                first.push_leaves(out);
                second.push_leaves(out);
            }
        }
    }
}

fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.1, 0.9)
    } else {
        0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_leaf_and_preserves_order() {
        let a = ViewId::next();
        let b = ViewId::next();
        let mut tree = PaneTree::leaf(a);

        assert!(tree.split_leaf(a, b, SplitAxis::Vertical, 0.65));
        assert_eq!(tree.leaves(), vec![a, b]);

        match tree {
            PaneTree::Split { axis, ratio, .. } => {
                assert_eq!(axis, SplitAxis::Vertical);
                assert_eq!(ratio, 0.65);
            }
            PaneTree::Leaf { .. } => panic!("expected split"),
        }
    }

    #[test]
    fn clamps_bad_ratios() {
        let a = ViewId::next();
        let b = ViewId::next();
        let c = ViewId::next();
        let mut tree = PaneTree::leaf(a);

        assert!(tree.split_leaf(a, b, SplitAxis::Horizontal, 12.0));
        assert!(tree.split_leaf(b, c, SplitAxis::Horizontal, f32::NAN));

        let PaneTree::Split { ratio, second, .. } = tree else {
            panic!("expected root split");
        };
        assert_eq!(ratio, 0.9);

        let PaneTree::Split { ratio, .. } = *second else {
            panic!("expected nested split");
        };
        assert_eq!(ratio, 0.5);
    }

    #[test]
    fn removes_leaf_and_promotes_sibling() {
        let a = ViewId::next();
        let b = ViewId::next();
        let c = ViewId::next();
        let mut tree = PaneTree::leaf(a);

        assert!(tree.split_leaf(a, b, SplitAxis::Vertical, 0.5));
        assert!(tree.split_leaf(b, c, SplitAxis::Horizontal, 0.5));
        assert_eq!(tree.leaves(), vec![a, b, c]);

        assert_eq!(tree.remove_leaf(b), Some(b));
        assert_eq!(tree.leaves(), vec![a, c]);
        assert_eq!(tree.remove_leaf(a), Some(a));
        assert_eq!(tree.leaves(), vec![c]);
    }
}
