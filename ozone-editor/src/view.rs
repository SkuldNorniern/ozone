use ozone_buffer::{BufferId, Pos, Span};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_VIEW_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ViewId(u64);

impl ViewId {
    pub fn next() -> Self {
        Self(NEXT_VIEW_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// A viewport into one buffer: scroll state, cursor, selection.
pub struct View {
    pub id: ViewId,
    pub buffer_id: BufferId,
    /// First visible line (0-indexed).
    pub scroll_line: usize,
    /// Pixel offset into `scroll_line`, used for smooth wheel scrolling.
    pub scroll_y: f32,
    /// Cursor position.
    pub cursor: Pos,
    /// Active selection span (None = no selection).
    pub selection: Option<Span>,
    /// Fixed end of a keyboard-extended selection; None when no selection is
    /// active. The cursor tracks the *moving* end; anchor tracks the *fixed*
    /// end. Text-object commands set anchor=span.start / cursor=span.end;
    /// extend commands keep anchor and move cursor only.
    pub anchor: Option<Pos>,
    /// Column memory for up/down movement across short lines.
    pub col_memory: usize,
    /// Visible line count — set by ozone-gui each frame so page commands work.
    pub page_height: usize,
    /// Header lines whose fold is collapsed (interior lines hidden). View-local,
    /// like Neovim window folds. See [`crate::fold`].
    pub folds: HashSet<usize>,
}

impl View {
    pub fn new(buffer_id: BufferId) -> Self {
        Self {
            id: ViewId::next(),
            buffer_id,
            scroll_line: 0,
            scroll_y: 0.0,
            cursor: Pos::zero(),
            selection: None,
            anchor: None,
            col_memory: 0,
            page_height: 40,
            folds: HashSet::new(),
        }
    }

    pub fn duplicate_for_split(&self) -> Self {
        Self {
            id: ViewId::next(),
            buffer_id: self.buffer_id,
            scroll_line: self.scroll_line,
            scroll_y: self.scroll_y,
            cursor: self.cursor,
            selection: self.selection,
            anchor: self.anchor,
            col_memory: self.col_memory,
            page_height: self.page_height,
            folds: self.folds.clone(),
        }
    }

    /// Clear both the selection span and the selection anchor.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.anchor = None;
    }

    /// Ensure the cursor is visible within `visible_lines` lines.
    pub fn scroll_to_cursor(&mut self, visible_lines: usize) {
        if self.cursor.line < self.scroll_line {
            self.scroll_line = self.cursor.line;
            self.scroll_y = 0.0;
        } else if self.cursor.line >= self.scroll_line + visible_lines {
            self.scroll_line = self.cursor.line + 1 - visible_lines;
            self.scroll_y = 0.0;
        }
    }

    pub fn scroll_by_pixels(&mut self, delta_px: f32, line_h: f32, max_scroll_line: usize) {
        if line_h <= 0.0 || delta_px == 0.0 {
            return;
        }
        let total = self.scroll_line as f32 * line_h + self.scroll_y + delta_px;
        let max_total = max_scroll_line as f32 * line_h;
        let clamped = total.clamp(0.0, max_total);
        self.scroll_line = (clamped / line_h).floor() as usize;
        self.scroll_y = clamped - self.scroll_line as f32 * line_h;
        if self.scroll_line >= max_scroll_line {
            self.scroll_line = max_scroll_line;
            self.scroll_y = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozone_buffer::BufferId;

    #[test]
    fn pixel_scroll_rolls_over_whole_lines() {
        let mut view = View::new(BufferId::next());
        view.scroll_by_pixels(27.0, 10.0, 10);
        assert_eq!(view.scroll_line, 2);
        assert_eq!(view.scroll_y, 7.0);
    }

    #[test]
    fn pixel_scroll_clamps_to_bounds() {
        let mut view = View::new(BufferId::next());
        view.scroll_by_pixels(1000.0, 10.0, 3);
        assert_eq!(view.scroll_line, 3);
        assert_eq!(view.scroll_y, 0.0);
        view.scroll_by_pixels(-1000.0, 10.0, 3);
        assert_eq!(view.scroll_line, 0);
        assert_eq!(view.scroll_y, 0.0);
    }
}
