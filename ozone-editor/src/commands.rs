use std::collections::HashMap;

use ozone_buffer::{BufferId, Pos};

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
        view.selection = None;
        if view.cursor.col > 0 {
            view.cursor.col -= 1;
        } else if view.cursor.line > 0 {
            view.cursor.line -= 1;
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            view.cursor.col = buf.line_len(view.cursor.line);
        }
        view.col_memory = view.cursor.col;
    });

    reg.register("cursor.move-right", "Move cursor one character right", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        let line_len = buf.line_len(view.cursor.line);
        if view.cursor.col < line_len {
            view.cursor.col += 1;
        } else if view.cursor.line + 1 < buf.line_count() {
            view.cursor.line += 1;
            view.cursor.col = 0;
        }
        view.col_memory = view.cursor.col;
    });

    reg.register("cursor.move-up", "Move cursor one line up", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        if view.cursor.line > 0 {
            view.cursor.line -= 1;
            let target_col = view.col_memory;
            view.cursor.col = target_col.min(buf.line_len(view.cursor.line));
        }
    });

    reg.register("cursor.move-down", "Move cursor one line down", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        if view.cursor.line + 1 < buf.line_count() {
            view.cursor.line += 1;
            let target_col = view.col_memory;
            view.cursor.col = target_col.min(buf.line_len(view.cursor.line));
        }
    });

    reg.register("cursor.line-start", "Move cursor to line start", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        view.cursor.col = 0;
        view.col_memory = 0;
    });

    reg.register("cursor.line-end", "Move cursor to line end", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        view.cursor.col = buf.line_len(view.cursor.line);
        view.col_memory = view.cursor.col;
    });

    reg.register("cursor.file-start", "Move cursor to file start", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        view.cursor = Pos::zero();
        view.col_memory = 0;
        view.scroll_line = 0;
    });

    reg.register("cursor.file-end", "Move cursor to file end", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.selection = None;
        let last_line = buf.line_count().saturating_sub(1);
        view.cursor = Pos::new(last_line, buf.line_len(last_line));
        view.col_memory = view.cursor.col;
    });

    // --- editing ---

    reg.register("edit.delete-char-backward", "Delete character before cursor", |ctx| {
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let cursor = view.cursor;
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();

        if cursor.col > 0 {
            let offset = buf.pos_to_offset(cursor) - 1;
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            buf.delete_at(offset, 1);
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            view.cursor.col -= 1;
            view.col_memory = view.cursor.col;
        } else if cursor.line > 0 {
            // Join with previous line
            let prev_line_len = buf.line_len(cursor.line - 1);
            let offset = buf.pos_to_offset(Pos::new(cursor.line - 1, prev_line_len));
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            buf.delete_at(offset, 1); // delete the '\n'
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            view.cursor = Pos::new(cursor.line - 1, prev_line_len);
            view.col_memory = view.cursor.col;
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
            buf.delete_at(offset, 1);
        }
    });

    reg.register("edit.insert-newline", "Insert a newline at cursor", |ctx| {
        let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
        let cursor = view.cursor;
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        buf.insert(cursor, "\n");
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.cursor = Pos::new(cursor.line + 1, 0);
        view.col_memory = 0;
    });

    reg.register("edit.undo", "Undo last edit", |ctx| {
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        if let Some(pos) = buf.undo() {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            view.cursor = pos;
            view.col_memory = pos.col;
        }
    });

    reg.register("edit.redo", "Redo last undone edit", |ctx| {
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        if let Some(pos) = buf.redo() {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            view.cursor = pos;
            view.col_memory = pos.col;
        }
    });

    // --- file ---

    reg.register("file.save", "Save the current buffer", |ctx| {
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        let _ = buf.save();
    });

    reg.register("file.save-all", "Save all dirty buffers", |ctx| {
        for buf in ctx.workspace.buffers.values_mut() {
            let _ = buf.save();
        }
    });
}
