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
    /// Lines of context kept above/below cursor when scrolling. Set by the GUI
    /// from `[editor] scroll_off`; defaults to 0 so the editor is usable before
    /// the GUI propagates it.
    pub scroll_off: usize,
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
            scroll_off: 0,
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
            scroll_off: self.scroll_off,
            folds: self.folds.clone(),
        }
    }

    /// Clear both the selection span and the selection anchor.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.anchor = None;
    }

    /// Ensure the cursor is visible within `visible_lines` lines, keeping at
    /// least `self.scroll_off` context lines above and below the cursor.
    pub fn scroll_to_cursor(&mut self, visible_lines: usize) {
        // Cap scroll_off so it can never push the cursor off-screen on its own
        // (e.g. tiny split panes or very large scroll_off values).
        let off = self.scroll_off.min(visible_lines.saturating_sub(1) / 2);
        let top_trigger = self.scroll_line + off;
        if self.cursor.line < top_trigger {
            self.scroll_line = self.cursor.line.saturating_sub(off);
            self.scroll_y = 0.0;
        } else {
            let bot_trigger = self.scroll_line + visible_lines;
            let cursor_bot = self.cursor.line + off + 1;
            if cursor_bot > bot_trigger {
                self.scroll_line = cursor_bot.saturating_sub(visible_lines);
                self.scroll_y = 0.0;
            }
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

    #[test]
    fn scroll_to_cursor_no_off_scrolls_exactly_to_edge() {
        let mut view = View::new(BufferId::next());
        // cursor moves below viewport
        view.cursor = Pos::new(20, 0);
        view.scroll_to_cursor(10);
        assert_eq!(view.scroll_line, 11); // 20 + 0 + 1 - 10 = 11
        // cursor moves above viewport
        view.cursor = Pos::new(5, 0);
        view.scroll_to_cursor(10);
        assert_eq!(view.scroll_line, 5);
    }

    #[test]
    fn scroll_to_cursor_with_scroll_off() {
        let mut view = View::new(BufferId::next());
        view.scroll_off = 3;
        // cursor moves below viewport (visible=10, off=3 → bot_trigger = scroll_line+10)
        view.scroll_line = 0;
        view.cursor = Pos::new(9, 0); // 9 + 3 + 1 = 13 > 10
        view.scroll_to_cursor(10);
        assert_eq!(view.scroll_line, 3); // 13 - 10 = 3
        // cursor near top — should pull scroll back
        view.scroll_line = 10;
        view.cursor = Pos::new(12, 0); // 12 < 10 + 3 = 13 → scroll_line = 12 - 3 = 9
        view.scroll_to_cursor(10);
        assert_eq!(view.scroll_line, 9);
    }

    #[test]
    fn scroll_off_capped_at_half_page() {
        let mut view = View::new(BufferId::next());
        // scroll_off=100 is capped to (10-1)/2 = 4.
        view.scroll_off = 100;
        view.cursor = Pos::new(9, 0);
        view.scroll_to_cursor(10);
        // bottom: cursor_bot = 9 + 4 + 1 = 14 > 0 + 10 → scroll_line = 14 - 10 = 4
        assert_eq!(view.scroll_line, 4);
    }
}
