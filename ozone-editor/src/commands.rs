use std::collections::HashMap;
use std::fs;

use ozone_buffer::{BufferId, BufferKind, Pos};

use crate::events::EditorEvent;
use crate::pane::{FocusDirection, SplitAxis};
use crate::ui::{NotifyLevel, UiIntent};
use crate::view::ViewId;
use crate::workspace::Workspace;

/// A short display name for a buffer (file name, or its virtual-kind label).
fn buffer_display_name(ws: &Workspace, id: BufferId) -> String {
    match ws.buffers.get(&id).map(|b| &b.kind) {
        Some(BufferKind::File(p)) => p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string(),
        Some(BufferKind::Scratch) => "*scratch*".to_string(),
        Some(BufferKind::Search) => "*files*".to_string(),
        Some(BufferKind::References) => "*references*".to_string(),
        Some(BufferKind::Terminal) => "*terminal*".to_string(),
        Some(BufferKind::Image(_)) => "*image*".to_string(),
        None => "?".to_string(),
    }
}

/// Everything a command needs to act on the editor state.
pub struct CommandContext<'a> {
    pub view_id: ViewId,
    pub buffer_id: BufferId,
    pub workspace: &'a mut Workspace,
    /// Optional string argument (e.g. text submitted to a prompting command).
    pub arg: Option<String>,
}

impl<'a> CommandContext<'a> {
    pub fn new(workspace: &'a mut Workspace) -> Option<Self> {
        let view_id = workspace.active_view_id?;
        let buffer_id = workspace.views.get(&view_id)?.buffer_id;
        Some(Self { view_id, buffer_id, workspace, arg: None })
    }

    /// Set the command argument (consumed by commands that take input).
    pub fn with_arg(mut self, arg: Option<String>) -> Self {
        self.arg = arg;
        self
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

    /// Human-friendly display name for UI (command palette). Derived from the id.
    pub fn display_name(&self, name: &str) -> String {
        pretty_command_name(name)
    }
}

/// Turn a command id into a display name: `"pane.focus-left"` -> `"Pane: Focus Left"`.
pub fn pretty_command_name(id: &str) -> String {
    fn title(seg: &str) -> String {
        seg.split(['-', '_'])
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    match id.split_once('.') {
        Some((group, rest)) => format!("{}: {}", title(group), title(rest)),
        None => title(id),
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
        ctx.workspace.push_jump();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor = Pos::zero();
        view.col_memory = 0;
        view.scroll_line = 0;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.file-end", "Move cursor to file end", |ctx| {
        ctx.workspace.push_jump();
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        let last_line = buf.line_count().saturating_sub(1);
        view.cursor = Pos::new(last_line, buf.line_len(last_line));
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register("view.jump-back", "Jump to the previous cursor location", |ctx| {
        ctx.workspace.jump_back();
    });

    reg.register("view.jump-forward", "Jump to the next cursor location", |ctx| {
        ctx.workspace.jump_forward();
    });

    reg.register("edit.goto-line", "Go to a line number", |ctx| {
        // No argument yet: prompt for one (re-invokes this command with the text).
        let Some(arg) = ctx.arg.clone() else {
            ctx.workspace.request_ui(UiIntent::Input {
                prompt: "go to line:".to_string(),
                command: "edit.goto-line".to_string(),
            });
            return;
        };
        let Ok(n) = arg.trim().parse::<usize>() else { return };
        let last = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap().line_count().saturating_sub(1);
        let line = n.saturating_sub(1).min(last); // 1-based input
        ctx.workspace.push_jump();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.selection = None;
        view.cursor = Pos::new(line, 0);
        view.col_memory = 0;
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

    reg.register("edit.insert-newline", "Insert a newline, preserving indentation", |ctx| {
        let cursor = ctx.workspace.views.get(&ctx.view_id).unwrap().cursor;
        let indent_unit = ctx.workspace.indent_for(ctx.buffer_id).unit();

        // Smart indent: copy the current line's leading whitespace, and add one
        // level when the text before the cursor ends with an opening bracket.
        let indent = {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let line_text = buf.line(cursor.line).unwrap_or_default();
            let lead = leading_whitespace(&line_text);
            let before = &line_text[..cursor.col.min(line_text.len())];
            let opens_block = before.trim_end().ends_with(['{', '(', '[']);
            if opens_block {
                format!("{lead}{indent_unit}")
            } else {
                lead.to_string()
            }
        };

        let insert = format!("\n{indent}");
        let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
        let delta = buf.insert(cursor, &insert);
        let cursor = {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            view.cursor = Pos::new(cursor.line + 1, indent.len());
            view.col_memory = view.cursor.col;
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
        let id = ctx.buffer_id;
        let name = buffer_display_name(ctx.workspace, id);
        match ctx.workspace.save_buffer(id) {
            Ok(()) => ctx.workspace.notify(NotifyLevel::Success, format!("Saved {name}")),
            Err(e) => ctx.workspace.notify(NotifyLevel::Error, format!("Save failed: {e}")),
        }
    });

    reg.register("file.save-all", "Save all dirty buffers", |ctx| {
        let ids: Vec<_> = ctx.workspace.buffers.keys().copied().collect();
        let mut saved = 0usize;
        for id in ids {
            match ctx.workspace.save_buffer(id) {
                Ok(()) => saved += 1,
                Err(e) => ctx.workspace.notify(NotifyLevel::Error, format!("Save failed: {e}")),
            }
        }
        ctx.workspace.notify(NotifyLevel::Success, format!("Saved {saved} buffer(s)"));
    });

    // Frontend-driven overlays. These are real commands (so they work from
    // keymaps, the palette, autocommands, and plugins), but the actual widget is
    // a GUI concern, so each just queues a `UiIntent` the frontend acts on.
    reg.register("command.palette", "Open the command palette", |ctx| {
        ctx.workspace.request_ui(UiIntent::CommandPalette);
    });
    reg.register("file.picker", "Open the workspace file picker", |ctx| {
        ctx.workspace.request_ui(UiIntent::FilePicker);
    });
    reg.register("buffer.picker", "Switch to an open buffer (fuzzy picker)", |ctx| {
        ctx.workspace.request_ui(UiIntent::BufferPicker);
    });
    reg.register("theme.select", "Select an installed color theme", |ctx| {
        ctx.workspace.request_ui(UiIntent::ThemePicker);
    });
    reg.register("theme.set", "Activate a color theme", |ctx| {
        if let Some(name) = ctx.arg.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
            ctx.workspace.request_ui(UiIntent::SetTheme { name: name.to_string() });
        } else {
            ctx.workspace.request_ui(UiIntent::ThemePicker);
        }
    });
    reg.register("search.start", "Incremental search in the buffer", |ctx| {
        ctx.workspace.request_ui(UiIntent::SearchStart);
    });
    reg.register("search.replace", "Search and replace in the buffer", |ctx| {
        ctx.workspace.request_ui(UiIntent::SearchReplace);
    });

    reg.register(
        "picker.open-selection",
        "Open the file on the picker's current line",
        |ctx| {
            // Only meaningful inside a picker (Search) buffer.
            let is_picker = matches!(
                ctx.workspace.buffers.get(&ctx.buffer_id).map(|b| &b.kind),
                Some(BufferKind::Search)
            );
            if !is_picker {
                return;
            }
            let line = ctx
                .workspace
                .views
                .get(&ctx.view_id)
                .and_then(|view| {
                    ctx.workspace
                        .buffers
                        .get(&ctx.buffer_id)
                        .and_then(|buf| buf.line(view.cursor.line))
                });
            let Some(line) = line else { return };
            let rel = line.trim();
            if rel.is_empty() {
                return;
            }
            let Ok(base) = std::env::current_dir() else {
                return;
            };
            let target = base.join(rel);
            if !target.is_file() {
                return;
            }
            let picker_view = ctx.view_id;
            if ctx.workspace.open_file(target).is_ok() {
                // The picker view was replaced in the active pane; drop it so the
                // transient buffer/view don't accumulate.
                ctx.workspace.discard_orphan_view(picker_view);
            }
        },
    );

    reg.register("terminal.open", "Open a terminal buffer placeholder", |ctx| {
        ctx.workspace.open_virtual_buffer(
            BufferKind::Terminal,
            "Terminal\n--------\nProcess-backed terminal buffers are planned next.\n".to_string(),
        );
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
        view.scroll_line = (view.scroll_line + page).min(max_scroll_line(line_count, page));
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
        let max = max_scroll_line(buf.line_count(), view.page_height);
        view.scroll_line = (view.scroll_line + 1).min(max);
    });

    reg.register("view.scroll-up", "Scroll view up one line", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.scroll_line = view.scroll_line.saturating_sub(1);
    });

    // --- panes ---

    reg.register("buffer.next", "Switch the active pane to the next buffer", |ctx| {
        ctx.workspace.cycle_buffer(true);
    });

    reg.register("buffer.previous", "Switch the active pane to the previous buffer", |ctx| {
        ctx.workspace.cycle_buffer(false);
    });

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

    reg.register("pane.focus-right", "Focus the pane to the right", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Right);
    });

    reg.register("pane.focus-down", "Focus the pane below", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Down);
    });

    reg.register("pane.focus-left", "Focus the pane to the left", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Left);
    });

    reg.register("pane.focus-up", "Focus the pane above", |ctx| {
        ctx.workspace.focus_pane_in_direction(FocusDirection::Up);
    });
}

fn max_scroll_line(line_count: usize, page_height: usize) -> usize {
    line_count.saturating_sub(page_height.max(1))
}

/// Names skipped when walking the workspace for the file picker.
fn is_ignored_name(name: &str) -> bool {
    matches!(name, "target" | "node_modules" | ".git" | ".hg" | ".svn")
        || name.starts_with('.')
}

/// Recursively collect file paths under `base`, relative and `/`-separated,
/// skipping VCS/build/hidden entries. Bounded by `cap`. No external crates.
pub fn collect_workspace_files(base: &std::path::Path, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if is_ignored_name(name) {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
                if out.len() >= cap {
                    break;
                }
            }
        }
    }
    out.sort();
    out
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

/// The leading run of spaces/tabs at the start of `line`.
fn leading_whitespace(line: &str) -> &str {
    let end = line
        .bytes()
        .position(|b| b != b' ' && b != b'\t')
        .unwrap_or(line.len());
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::{collect_workspace_files, is_ignored_name, trailing_whitespace_ranges};

    #[test]
    fn finds_trailing_space_ranges() {
        assert_eq!(
            trailing_whitespace_ranges("a  \nb\t\nc\n  "),
            vec![(1, 2), (5, 1), (9, 2)]
        );
    }

    #[test]
    fn ignores_vcs_build_and_hidden() {
        assert!(is_ignored_name("target"));
        assert!(is_ignored_name(".git"));
        assert!(is_ignored_name(".hidden"));
        assert!(!is_ignored_name("src"));
        assert!(!is_ignored_name("Cargo.toml"));
    }

    #[test]
    fn collect_walks_recursively_skipping_ignored() {
        let base = std::env::temp_dir().join(format!("ozone_pick_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::create_dir_all(base.join("target")).unwrap();
        std::fs::write(base.join("Cargo.toml"), "x").unwrap();
        std::fs::write(base.join("src").join("main.rs"), "x").unwrap();
        std::fs::write(base.join("target").join("junk.o"), "x").unwrap();

        let files = collect_workspace_files(&base, 5000);
        assert!(files.contains(&"Cargo.toml".to_string()));
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(!files.iter().any(|f| f.contains("target")));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn collect_respects_cap() {
        let base = std::env::temp_dir().join(format!("ozone_cap_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        for i in 0..10 {
            std::fs::write(base.join(format!("f{i}.txt")), "x").unwrap();
        }
        let files = collect_workspace_files(&base, 3);
        assert!(files.len() <= 3);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn goto_line_jumps_to_argument() {
        use super::{CommandContext, CommandRegistry, register_defaults};
        use crate::workspace::Workspace;
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text("a\nb\nc\nd\ne");
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        // 1-based line 3 -> index 2.
        let mut ctx = CommandContext::new(&mut ws).unwrap().with_arg(Some("3".to_string()));
        assert!(reg.execute("edit.goto-line", &mut ctx));
        assert_eq!(ws.active_view().unwrap().cursor.line, 2);
        // Out-of-range clamps to last line.
        let mut ctx = CommandContext::new(&mut ws).unwrap().with_arg(Some("999".to_string()));
        reg.execute("edit.goto-line", &mut ctx);
        assert_eq!(ws.active_view().unwrap().cursor.line, 4);
    }

    #[test]
    fn pretty_command_names() {
        use super::pretty_command_name;
        assert_eq!(pretty_command_name("pane.focus-left"), "Pane: Focus Left");
        assert_eq!(pretty_command_name("file.save"), "File: Save");
        assert_eq!(pretty_command_name("command.palette"), "Command: Palette");
        assert_eq!(pretty_command_name("buffer.next"), "Buffer: Next");
    }

    #[test]
    fn leading_whitespace_extracts_indent() {
        assert_eq!(super::leading_whitespace("    foo"), "    ");
        assert_eq!(super::leading_whitespace("\t\tbar"), "\t\t");
        assert_eq!(super::leading_whitespace("none"), "");
        assert_eq!(super::leading_whitespace("   "), "   ");
    }

    fn run_newline(content: &str, cursor_col: usize) -> (Option<String>, ozone_buffer::Pos) {
        use crate::{CommandRegistry, Workspace};
        use ozone_buffer::Pos;
        let mut reg = CommandRegistry::new();
        super::register_defaults(&mut reg);
        let mut ws = Workspace::new();
        let buf_id = ws.active_buffer().unwrap().id;
        let view_id = ws.active_view_id.unwrap();
        ws.buffers.get_mut(&buf_id).unwrap().insert(Pos::new(0, 0), content);
        ws.views.get_mut(&view_id).unwrap().cursor = Pos::new(0, cursor_col);
        let mut ctx = super::CommandContext::new(&mut ws).unwrap();
        reg.execute("edit.insert-newline", &mut ctx);
        let line1 = ws.buffers.get(&buf_id).unwrap().line(1);
        (line1, ws.active_view().unwrap().cursor)
    }

    #[test]
    fn newline_preserves_indentation() {
        let (line1, cursor) = run_newline("    foo", 7);
        assert_eq!(line1.as_deref(), Some("    "));
        assert_eq!(cursor, ozone_buffer::Pos::new(1, 4));
    }

    #[test]
    fn newline_adds_level_after_opening_brace() {
        // default indent is 4 soft spaces; line has no lead but ends with '{'
        let (line1, cursor) = run_newline("fn x() {", 8);
        assert_eq!(line1.as_deref(), Some("    "));
        assert_eq!(cursor, ozone_buffer::Pos::new(1, 4));
    }

    #[test]
    fn newline_plain_line_has_no_indent() {
        let (line1, cursor) = run_newline("hello", 5);
        assert_eq!(line1.as_deref(), Some(""));
        assert_eq!(cursor, ozone_buffer::Pos::new(1, 0));
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
