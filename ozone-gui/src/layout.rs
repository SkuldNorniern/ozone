//! Shared editor geometry: layout constants + the rect math used by *both* the
//! renderer and mouse hit-testing.
//!
//! Rendering (`draw_*`) and pointer hit-testing (`pane_at`, click→cursor) must
//! agree on pane rects, gutter width, and the status-bar height down to the
//! pixel, or a click lands on the wrong glyph. Keeping that math in one module
//! (rather than duplicated across `render` and `mouse`) is what makes those two
//! safe to split apart.

use aurea::render::Rect;
use ozone_config::LineNumbers;
use ozone_editor::{PaneTree, SplitAxis, ViewId};

// Layout constants (px).
pub(crate) const GUTTER_MIN_W: f32 = 52.0;
pub(crate) const PAD: f32 = 8.0;
pub(crate) const STATUS_H: f32 = 28.0;
pub(crate) const EDITOR_TOP_PAD: f32 = 10.0;
pub(crate) const SPLIT_GAP: f32 = 4.0;

/// Vertically centre a single text line's baseline inside a rect of `height`.
pub(crate) fn baseline_in_rect(top: f32, height: f32, ascent: f32, descent: f32) -> f32 {
    top + (height + ascent - descent) / 2.0
}

/// Gutter width for `line_count` lines, or 0 when line numbers are off.
pub(crate) fn gutter_width(line_count: usize, char_w: f32, mode: LineNumbers) -> f32 {
    if mode == LineNumbers::Off {
        return 0.0;
    }
    let digits = line_count.max(1).to_string().len().max(2);
    GUTTER_MIN_W.max((digits as f32 + 2.0) * char_w + PAD)
}

/// Whether `(x, y)` falls inside `rect` (half-open on the far edges).
pub(crate) fn point_in_rect(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

/// The leaf view (and its rect) containing `(x, y)`, walking the pane tree.
pub(crate) fn pane_at(tree: &PaneTree, rect: Rect, x: f32, y: f32) -> Option<(ViewId, Rect)> {
    if !point_in_rect(rect, x, y) {
        return None;
    }
    match tree {
        PaneTree::Leaf { view_id } => Some((*view_id, rect)),
        PaneTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (first_rect, second_rect, _) = split_rect(rect, *axis, *ratio);
            pane_at(first, first_rect, x, y).or_else(|| pane_at(second, second_rect, x, y))
        }
    }
}

/// Split `rect` by `axis`/`ratio` into `(first, second, divider)` rects.
pub(crate) fn split_rect(rect: Rect, axis: SplitAxis, ratio: f32) -> (Rect, Rect, Rect) {
    let ratio = ratio.clamp(0.1, 0.9);
    match axis {
        SplitAxis::Vertical => {
            let first_w = (rect.width * ratio - SPLIT_GAP / 2.0).max(0.0);
            let divider_x = rect.x + first_w;
            let second_x = divider_x + SPLIT_GAP;
            let second_w = (rect.x + rect.width - second_x).max(0.0);
            (
                Rect::new(rect.x, rect.y, first_w, rect.height),
                Rect::new(second_x, rect.y, second_w, rect.height),
                Rect::new(divider_x, rect.y, SPLIT_GAP, rect.height),
            )
        }
        SplitAxis::Horizontal => {
            let first_h = (rect.height * ratio - SPLIT_GAP / 2.0).max(0.0);
            let divider_y = rect.y + first_h;
            let second_y = divider_y + SPLIT_GAP;
            let second_h = (rect.y + rect.height - second_y).max(0.0);
            (
                Rect::new(rect.x, rect.y, rect.width, first_h),
                Rect::new(rect.x, second_y, rect.width, second_h),
                Rect::new(rect.x, divider_y, rect.width, SPLIT_GAP),
            )
        }
    }
}

/// The largest scroll line that still keeps content on screen.
pub(crate) fn max_scroll_line(line_count: usize, page_height: usize) -> usize {
    line_count.saturating_sub(page_height.max(1))
}
