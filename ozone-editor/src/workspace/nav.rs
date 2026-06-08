use ozone_buffer::{BufferId, BufferKind, Pos};

use crate::events::EditorEvent;

use super::{JUMP_CAP, Loc, Workspace};

impl Workspace {
    /// Switch the active pane to the next/previous open buffer (by id order,
    /// wrapping). Repoints the active view and clamps its cursor to the new
    /// buffer. Returns true if the buffer changed.
    pub fn cycle_buffer(&mut self, forward: bool) -> bool {
        let mut ids: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buf)| matches!(buf.kind, BufferKind::File(_) | BufferKind::Scratch))
            .map(|(id, _)| *id)
            .collect();
        if ids.len() <= 1 {
            return false;
        }
        ids.sort_by_key(|id| id.raw());

        let Some(view_id) = self.active_view_id else {
            return false;
        };
        let Some(current) = self.views.get(&view_id).map(|v| v.buffer_id) else {
            return false;
        };
        let idx = ids.iter().position(|id| *id == current).unwrap_or(0);
        let next = if forward {
            (idx + 1) % ids.len()
        } else {
            (idx + ids.len() - 1) % ids.len()
        };
        let target = ids[next];
        if target == current {
            return false;
        }

        let (last_line, line_len) = {
            let buf = match self.buffers.get(&target) {
                Some(buf) => buf,
                None => return false,
            };
            let last_line = buf.line_count().saturating_sub(1);
            (last_line, buf.line_len(last_line))
        };

        let cursor = if let Some(view) = self.views.get_mut(&view_id) {
            view.buffer_id = target;
            view.cursor.line = view.cursor.line.min(last_line);
            view.cursor.col = view.cursor.col.min(line_len);
            view.col_memory = view.cursor.col;
            view.scroll_line = 0;
            view.scroll_y = 0.0;
            Some((view.id, view.cursor))
        } else {
            None
        };
        if let Some((id, pos)) = cursor {
            self.emit(EditorEvent::CursorMoved { view_id: id, pos });
        }
        true
    }

    fn current_loc(&self) -> Option<Loc> {
        let view = self.active_view()?;
        Some((view.buffer_id, view.cursor))
    }

    /// Record the active location as a jump origin (clears the forward stack).
    /// Call before a navigation that should be reachable with `jump_back`.
    pub fn push_jump(&mut self) {
        if let Some(loc) = self.current_loc() {
            if self.jumps.back.last() != Some(&loc) {
                self.jumps.back.push(loc);
                if self.jumps.back.len() > JUMP_CAP {
                    self.jumps.back.remove(0);
                }
            }
            self.jumps.fwd.clear();
        }
    }

    /// Jump to the most recent recorded origin (Ctrl+-). Returns false if there
    /// is nowhere to go back to (skipping origins whose buffer was closed).
    pub fn jump_back(&mut self) -> bool {
        let Some(cur) = self.current_loc() else {
            return false;
        };
        while let Some(target) = self.jumps.back.pop() {
            if self.buffers.contains_key(&target.0) {
                self.jumps.fwd.push(cur);
                return self.apply_loc(target.0, target.1);
            }
        }
        false
    }

    /// Re-do a jump undone by `jump_back`. Returns false if there is none.
    pub fn jump_forward(&mut self) -> bool {
        let Some(cur) = self.current_loc() else {
            return false;
        };
        while let Some(target) = self.jumps.fwd.pop() {
            if self.buffers.contains_key(&target.0) {
                self.jumps.back.push(cur);
                return self.apply_loc(target.0, target.1);
            }
        }
        false
    }

    /// Point the active view at `buffer_id`, placing the cursor at `pos`
    /// (clamped) and scrolling it into view. Returns false if the buffer is gone.
    fn apply_loc(&mut self, buffer_id: BufferId, pos: Pos) -> bool {
        let Some(buf) = self.buffers.get(&buffer_id) else {
            return false;
        };
        let last_line = buf.line_count().saturating_sub(1);
        let line = pos.line.min(last_line);
        let col = pos.col.min(buf.line_len(line));
        let Some(view_id) = self.active_view_id else {
            return false;
        };
        let Some(view) = self.views.get_mut(&view_id) else {
            return false;
        };
        view.buffer_id = buffer_id;
        view.cursor = Pos::new(line, col);
        view.col_memory = col;
        view.selection = None;
        view.scroll_to_cursor(view.page_height.max(1));
        let (id, cursor) = (view.id, view.cursor);
        self.emit(EditorEvent::CursorMoved {
            view_id: id,
            pos: cursor,
        });
        true
    }

    /// Switch the active pane to a specific open buffer (used by the buffer
    /// picker). Records a jump so Ctrl+- returns to the previous buffer.
    pub fn switch_active_buffer(&mut self, target: BufferId) -> bool {
        if !self.buffers.contains_key(&target) {
            return false;
        }
        if self.current_loc().map(|l| l.0) == Some(target) {
            return false;
        }
        self.push_jump();
        self.apply_loc(target, Pos::zero())
    }
}
