use ozone_buffer::{BufferId, Pos, Span};
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
    /// Cursor position.
    pub cursor: Pos,
    /// Active selection anchor (None = no selection).
    pub selection: Option<Span>,
    /// Column memory for up/down movement across short lines.
    pub col_memory: usize,
}

impl View {
    pub fn new(buffer_id: BufferId) -> Self {
        Self {
            id: ViewId::next(),
            buffer_id,
            scroll_line: 0,
            cursor: Pos::zero(),
            selection: None,
            col_memory: 0,
        }
    }

    /// Ensure the cursor is visible within `visible_lines` lines.
    pub fn scroll_to_cursor(&mut self, visible_lines: usize) {
        if self.cursor.line < self.scroll_line {
            self.scroll_line = self.cursor.line;
        } else if self.cursor.line >= self.scroll_line + visible_lines {
            self.scroll_line = self.cursor.line + 1 - visible_lines;
        }
    }
}
