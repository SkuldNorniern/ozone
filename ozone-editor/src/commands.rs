use std::collections::HashMap;

use ozone_buffer::{BufferId, Pos};

use crate::events::EditorEvent;
use crate::pane::SplitAxis;
use crate::view::ViewId;
use crate::workspace::Workspace;

/// Everything a command needs to act on the editor state.
pub struct CommandContext<'a> {
    pub view_id: ViewId,
    pub buffer_id: BufferId,
    pub workspace: &'a mut Workspace,
}

impl<'a> CommandContext<'a> {
    pub fn new(workspace: &'a mut Workspace) -> Option<Self> {
        let view_id = workspace.active_view_id?;
        let buffer_id = workspace.views.get(&view_id)?.buffer_id;
        Some(Self { view_id, buffer_id, workspace })
    }
}

type CommandFn = Box<dyn Fn(&mut CommandContext) + Send + Sync>;

/// Maps command names → handlers. Everything is a command.
pub struct CommandRegistry {
    commands: HashMap<String, CommandFn>,
    descriptions: HashMap<String, String>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self { commands: HashMap::new(), descriptions: HashMap::new() }
    }

    pub fn register(&mut self, name: &str, description: &str, f: impl Fn(&mut CommandContext) + Send + Sync + 'static) {
        self.commands.insert(name.to_string(), Box::new(f));
        self.descriptions.insert(name.to_string(), description.to_string());
    }

    /// Returns true if the command existed.
    pub fn execute(&self, name: &str, ctx: &mut CommandContext) -> bool {
        if let Some(cmd) = self.commands.get(name) {
            cmd(ctx);
            true
        } else {
            false
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(String::as_str)
    }

    pub fn description(&self, name: &str) -> Option<&str> {
        self.descriptions.get(name).map(String::as_str)
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Register all Phase-0 built-in commands.
pub fn register_defaults(reg: &mut CommandRegistry) {
    // --- cursor movement ---

    reg.register("cursor.move-left", "Move cursor one character left", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        if view.cursor.col > 0 {
            view.cursor.col -= 1;
        } else if view.cursor.line > 0 {
            view.cursor.line -= 1;
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            view.cursor.col = buf.line_len(view.cursor.line);
        }
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.move-right", "Move cursor one character right", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        let line_len = buf.line_len(view.cursor.line);
        if view.cursor.col < line_len {
            view.cursor.col += 1;
        } else if view.cursor.line + 1 < buf.line_count() {
            view.cursor.line += 1;
            view.cursor.col = 0;
        }
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.move-up", "Move cursor one line up", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        if view.cursor.line > 0 {
            view.cursor.line -= 1;
            let target_col = view.col_memory;
            view.cursor.col = target_col.min(buf.line_len(view.cursor.line));
        }
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.move-down", "Move cursor one line down", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        if view.cursor.line + 1 < buf.line_count() {
            view.cursor.line += 1;
            let target_col = view.col_memory;
            view.cursor.col = target_col.min(buf.line_len(view.cursor.line));
        }
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.line-start", "Move cursor to line start", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor.col = 0;
        view.col_memory = 0;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.line-end", "Move cursor to line end", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor.col = buf.line_len(view.cursor.line);
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.file-start", "Move cursor to file start", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor = Pos::zero();
        view.col_memory = 0;
        view.scroll_line = 0;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.file-end", "Move cursor to file end", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        let last_line = buf.line_count().saturating_sub(1);
        view.cursor = Pos::new(last_line, buf.line_len(last_line));
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    // --- editing ---

    reg.register("edit.delete-char-backward", "Delete character before cursor", |ctx| {
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let cursor = view.cursor;
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();

        if cursor.col > 0 {
            let offset = buf.pos_to_offset(cursor) - 1;
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            let delta = buf.delete_at(offset, 1);
            let cursor = {
                let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
                view.cursor.col -= 1;
                view.col_memory = view.cursor.col;
                view.cursor
            };
            if let Some(delta) = delta {
                ctx.workspace.emit(EditorEvent::BufferChanged { id: ctx.buffer_id, delta });
            }
            ctx.workspace.emit(EditorEvent::CursorMoved { view_id: ctx.view_id, pos: cursor });
        } else if cursor.line > 0 {
            // Join with previous line
            let prev_line_len = buf.line_len(cursor.line - 1);
            let offset = buf.pos_to_offset(Pos::new(cursor.line - 1, prev_line_len));
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            let delta = buf.delete_at(offset, 1); // delete the '\n'
            let cursor = {
                let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
                view.cursor = Pos::new(cursor.line - 1, prev_line_len);
                view.col_memory = view.cursor.col;
                view.cursor
            };
            if let Some(delta) = delta {
                ctx.workspace.emit(EditorEvent::BufferChanged { id: ctx.buffer_id, delta });
            }
            ctx.workspace.emit(EditorEvent::CursorMoved { view_id: ctx.view_id, pos: cursor });
        }
    });

    reg.register("edit.delete-char-forward", "Delete character after cursor", |ctx| {
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let cursor = view.cursor;
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let offset = buf.pos_to_offset(cursor);
        let total = buf.text().len();
        if offset < total {
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            if let Some(delta) = buf.delete_at(offset, 1) {
                ctx.workspace.emit(EditorEvent::BufferChanged { id: ctx.buffer_id, delta });
            }
        }
    });

    reg.register("edit.insert-newline", "Insert a newline at cursor", |ctx| {
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let cursor = view.cursor;
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        let delta = buf.insert(cursor, "\n");
        let cursor = {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            view.cursor = Pos::new(cursor.line + 1, 0);
            view.col_memory = 0;
            view.cursor
        };
        ctx.workspace.emit(EditorEvent::BufferChanged { id: ctx.buffer_id, delta });
        ctx.workspace.emit(EditorEvent::CursorMoved { view_id: ctx.view_id, pos: cursor });
    });

    reg.register("edit.undo", "Undo last edit", |ctx| {
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        if let Some(pos) = buf.undo() {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.cursor = pos;
            view.col_memory = pos.col;
            emit_cursor_moved(ctx, old);
        }
    });

    reg.register("edit.redo", "Redo last undone edit", |ctx| {
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        if let Some(pos) = buf.redo() {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.cursor = pos;
            view.col_memory = pos.col;
            emit_cursor_moved(ctx, old);
        }
    });

    reg.register("edit.trim-trailing-whitespace", "Trim trailing spaces and tabs", |ctx| {
        let ranges = {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            trailing_whitespace_ranges(&buf.text())
        };
        if ranges.is_empty() {
            return;
        }

        let mut deltas = Vec::new();
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        for (offset, len) in ranges.into_iter().rev() {
            if let Some(delta) = buf.delete_at(offset, len) {
                deltas.push(delta);
            }
        }
        for delta in deltas {
            ctx.workspace.emit(EditorEvent::BufferChanged { id: ctx.buffer_id, delta });
        }
    });

    // --- file ---

    reg.register("file.save", "Save the current buffer", |ctx| {
        let _ = ctx.workspace.save_buffer(ctx.buffer_id);
    });

    reg.register("file.save-all", "Save all dirty buffers", |ctx| {
        let ids: Vec<_> = ctx.workspace.buffers.keys().copied().collect();
        for id in ids {
            let _ = ctx.workspace.save_buffer(id);
        }
    });

    // --- word movement ---

    reg.register("cursor.word-forward", "Move cursor to start of next word", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let pos = word_forward(buf, view.cursor);
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor = pos;
        view.col_memory = pos.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.word-backward", "Move cursor to start of previous word", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let pos = word_backward(buf, view.cursor);
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor = pos;
        view.col_memory = pos.col;
        emit_cursor_moved(ctx, old);
    });

    // --- page movement ---

    reg.register("view.page-down", "Scroll down one page", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let line_count = buf.line_count();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        let page = view.page_height.max(1);
        view.cursor.line = (view.cursor.line + page).min(line_count.saturating_sub(1));
        view.cursor.col = view.cursor.col.min(buf.line_len(view.cursor.line));
        view.col_memory = view.cursor.col;
        view.scroll_line = (view.scroll_line + page).min(line_count.saturating_sub(1));
        emit_cursor_moved(ctx, old);
    });

    reg.register("view.page-up", "Scroll up one page", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        let page = view.page_height.max(1);
        view.cursor.line = view.cursor.line.saturating_sub(page);
        view.cursor.col = view.cursor.col.min(buf.line_len(view.cursor.line));
        view.col_memory = view.cursor.col;
        view.scroll_line = view.scroll_line.saturating_sub(page);
        emit_cursor_moved(ctx, old);
    });

    // --- view scroll (without cursor move) ---

    reg.register("view.scroll-down", "Scroll view down one line", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let max = buf.line_count().saturating_sub(1);
        view.scroll_line = (view.scroll_line + 1).min(max);
    });

    reg.register("view.scroll-up", "Scroll view up one line", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.scroll_line = view.scroll_line.saturating_sub(1);
    });

    // --- panes ---

    reg.register("pane.split-right", "Split the active pane vertically", |ctx| {
        ctx.workspace.split_active_pane(SplitAxis::Vertical);
    });

    reg.register("pane.split-down", "Split the active pane horizontally", |ctx| {
        ctx.workspace.split_active_pane(SplitAxis::Horizontal);
    });

    reg.register("pane.close", "Close the active pane", |ctx| {
        ctx.workspace.close_view(ctx.view_id);
    });

    reg.register("pane.focus-next", "Focus the next pane", |ctx| {
        ctx.workspace.focus_next_pane();
    });

    reg.register("pane.focus-previous", "Focus the previous pane", |ctx| {
        ctx.workspace.focus_previous_pane();
    });
}

fn emit_cursor_moved(ctx: &mut CommandContext, old: Pos) {
    let Some(view) = ctx.workspace.views.get(&ctx.view_id) else {
        return;
    };
    if view.cursor != old {
        ctx.workspace.emit(EditorEvent::CursorMoved { view_id: ctx.view_id, pos: view.cursor });
    }
}

fn trailing_whitespace_ranges(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut line_start = 0usize;
    let mut i = 0usize;

    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            let line_end = i;
            let mut trim_start = line_end;
            while trim_start > line_start && matches!(bytes[trim_start - 1], b' ' | b'\t') {
                trim_start -= 1;
            }
            if trim_start < line_end {
                ranges.push((trim_start, line_end - trim_start));
            }
            line_start = i + 1;
        }
        i += 1;
    }

    ranges
}

// ---------------------------------------------------------------------------
// Word-movement helpers
// ---------------------------------------------------------------------------

fn is_word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

#[cfg(test)]
mod tests {
    use super::trailing_whitespace_ranges;

    #[test]
    fn finds_trailing_space_ranges() {
        assert_eq!(
            trailing_whitespace_ranges("a  \nb\t\nc\n  "),
            vec![(1, 2), (5, 1), (9, 2)]
        );
    }
}

fn word_forward(buf: &ozone_buffer::Buffer, pos: Pos) -> Pos {
    let line_count = buf.line_count();
    let mut line = pos.line;
    let mut col = pos.col;

    loop {
        let line_text = match buf.line(line) {
            Some(t) => t,
            None => return Pos::new(line_count.saturating_sub(1), 0),
        };
        let bytes = line_text.as_bytes();

        // Skip current word chars
        while col < bytes.len() && is_word_char(bytes[col]) {
            col += 1;
        }
        // Skip non-word chars
        while col < bytes.len() && !is_word_char(bytes[col]) {
            col += 1;
        }

        if col < bytes.len() {
            return Pos::new(line, col);
        }

        // Move to next line
        if line + 1 < line_count {
            line += 1;
            col = 0;
        } else {
            return Pos::new(line, bytes.len());
        }
    }
}

fn word_backward(buf: &ozone_buffer::Buffer, pos: Pos) -> Pos {
    let mut line = pos.line;
    let mut col = pos.col;

    loop {
        let line_text = match buf.line(line) {
            Some(t) => t,
            None => return Pos::zero(),
        };
        let bytes = line_text.as_bytes();

        if col == 0 {
            if line == 0 { return Pos::zero(); }
            line -= 1;
            col = buf.line_len(line);
            continue;
        }

        col -= 1;
        // Skip non-word chars going left
        while col > 0 && !is_word_char(bytes[col]) {
            col -= 1;
        }
        // Skip word chars going left to find start
        while col > 0 && is_word_char(bytes[col - 1]) {
            col -= 1;
        }
        return Pos::new(line, col);
    }
}
