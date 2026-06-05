use crate::view::ViewId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
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

    pub fn replace_leaf(&mut self, target: ViewId, replacement: ViewId) -> bool {
        match self {
            Self::Leaf { view_id } if *view_id == target => {
                *view_id = replacement;
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.replace_leaf(target, replacement) || second.replace_leaf(target, replacement)
            }
        }
    }

    pub fn leaves(&self) -> Vec<ViewId> {
        let mut out = Vec::new();
        self.push_leaves(&mut out);
        out
    }

    pub fn next_leaf_after(&self, current: ViewId) -> Option<ViewId> {
        let leaves = self.leaves();
        let idx = leaves.iter().position(|id| *id == current)?;
        Some(leaves[(idx + 1) % leaves.len()])
    }

    pub fn previous_leaf_before(&self, current: ViewId) -> Option<ViewId> {
        let leaves = self.leaves();
        let idx = leaves.iter().position(|id| *id == current)?;
        Some(leaves[(idx + leaves.len() - 1) % leaves.len()])
    }

    pub fn neighbor_in_direction(&self, current: ViewId, direction: FocusDirection) -> Option<ViewId> {
        let mut rects = Vec::new();
        self.push_rects(UnitRect::new(0.0, 0.0, 1.0, 1.0), &mut rects);
        let current_rect = rects.iter().find(|(id, _)| *id == current)?.1;
        let (cx, cy) = current_rect.center();

        rects
            .into_iter()
            .enumerate()
            .filter(|(_, (id, _))| *id != current)
            .filter_map(|(order, (id, rect))| {
                let (tx, ty) = rect.center();
                let primary = match direction {
                    FocusDirection::Left if tx < cx => cx - tx,
                    FocusDirection::Right if tx > cx => tx - cx,
                    FocusDirection::Up if ty < cy => cy - ty,
                    FocusDirection::Down if ty > cy => ty - cy,
                    _ => return None,
                };
                let secondary = match direction {
                    FocusDirection::Left | FocusDirection::Right => (ty - cy).abs(),
                    FocusDirection::Up | FocusDirection::Down => (tx - cx).abs(),
                };
                let overlaps = match direction {
                    FocusDirection::Left | FocusDirection::Right => current_rect.overlaps_y(rect),
                    FocusDirection::Up | FocusDirection::Down => current_rect.overlaps_x(rect),
                };
                let overlap_penalty = if overlaps { 0.0 } else { 10_000.0 };
                Some((id, overlap_penalty + primary * 1000.0 + secondary + order as f32 * 0.0001))
            })
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(id, _)| id)
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

    fn push_rects(&self, rect: UnitRect, out: &mut Vec<(ViewId, UnitRect)>) {
        match self {
            Self::Leaf { view_id } => out.push((*view_id, rect)),
            Self::Split { axis, ratio, first, second } => {
                let (first_rect, second_rect) = rect.split(*axis, *ratio);
                first.push_rects(first_rect, out);
                second.push_rects(second_rect, out);
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

#[derive(Debug, Clone, Copy)]
struct UnitRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl UnitRect {
    fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn split(self, axis: SplitAxis, ratio: f32) -> (Self, Self) {
        let ratio = clamp_ratio(ratio);
        match axis {
            SplitAxis::Vertical => {
                let first_w = self.width * ratio;
                (
                    Self::new(self.x, self.y, first_w, self.height),
                    Self::new(self.x + first_w, self.y, self.width - first_w, self.height),
                )
            }
            SplitAxis::Horizontal => {
                let first_h = self.height * ratio;
                (
                    Self::new(self.x, self.y, self.width, first_h),
                    Self::new(self.x, self.y + first_h, self.width, self.height - first_h),
                )
            }
        }
    }

    fn overlaps_x(self, other: Self) -> bool {
        self.x < other.x + other.width && other.x < self.x + self.width
    }

    fn overlaps_y(self, other: Self) -> bool {
        self.y < other.y + other.height && other.y < self.y + self.height
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

    #[test]
    fn focus_order_wraps_through_leaves() {
        let a = ViewId::next();
        let b = ViewId::next();
        let c = ViewId::next();
        let mut tree = PaneTree::leaf(a);

        assert!(tree.split_leaf(a, b, SplitAxis::Vertical, 0.5));
        assert!(tree.split_leaf(b, c, SplitAxis::Horizontal, 0.5));

        assert_eq!(tree.next_leaf_after(a), Some(b));
        assert_eq!(tree.next_leaf_after(b), Some(c));
        assert_eq!(tree.next_leaf_after(c), Some(a));
        assert_eq!(tree.previous_leaf_before(a), Some(c));
        assert_eq!(tree.previous_leaf_before(c), Some(b));
    }

    #[test]
    fn replaces_a_leaf_without_changing_split_shape() {
        let a = ViewId::next();
        let b = ViewId::next();
        let c = ViewId::next();
        let mut tree = PaneTree::leaf(a);

        assert!(tree.split_leaf(a, b, SplitAxis::Vertical, 0.5));
        assert!(tree.replace_leaf(b, c));

        assert_eq!(tree.leaves(), vec![a, c]);
        assert!(!tree.replace_leaf(b, a));
    }

    #[test]
    fn directional_focus_uses_pane_geometry() {
        let left = ViewId::next();
        let top_right = ViewId::next();
        let bottom_right = ViewId::next();
        let mut tree = PaneTree::leaf(left);

        assert!(tree.split_leaf(left, top_right, SplitAxis::Vertical, 0.5));
        assert!(tree.split_leaf(top_right, bottom_right, SplitAxis::Horizontal, 0.5));

        assert_eq!(tree.neighbor_in_direction(left, FocusDirection::Right), Some(top_right));
        assert_eq!(tree.neighbor_in_direction(top_right, FocusDirection::Down), Some(bottom_right));
        assert_eq!(tree.neighbor_in_direction(bottom_right, FocusDirection::Up), Some(top_right));
        assert_eq!(tree.neighbor_in_direction(top_right, FocusDirection::Left), Some(left));
        assert_eq!(tree.neighbor_in_direction(left, FocusDirection::Left), None);
    }
}
