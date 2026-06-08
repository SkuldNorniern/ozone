use std::collections::HashMap;
use std::fs;

use ozone_buffer::{BufferId, BufferKind, Pos};

use crate::events::EditorEvent;
use crate::view::ViewId;
use crate::workspace::Workspace;

mod cursor;
mod edit;
mod file;
mod pane;

use cursor::register_cursor_commands;
use edit::register_edit_commands;
use file::register_file_commands;
use pane::register_pane_commands;

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
        Some(BufferKind::FileTree) => "*tree*".to_string(),
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
        Some(Self {
            view_id,
            buffer_id,
            workspace,
            arg: None,
        })
    }

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
        Self {
            commands: HashMap::new(),
            descriptions: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        f: impl Fn(&mut CommandContext) + Send + Sync + 'static,
    ) {
        self.commands.insert(name.to_string(), Box::new(f));
        self.descriptions
            .insert(name.to_string(), description.to_string());
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

    pub fn display_name(&self, name: &str) -> String {
        pretty_command_name(name)
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
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

/// Register all built-in commands.
pub fn register_defaults(reg: &mut CommandRegistry) {
    register_cursor_commands(reg);
    register_edit_commands(reg);
    register_file_commands(reg);
    register_pane_commands(reg);
}

// ---------------------------------------------------------------------------
// Shared helpers — pub(super) so sub-modules can use them via `use super::*`
// ---------------------------------------------------------------------------

pub(super) fn max_scroll_line(line_count: usize, page_height: usize) -> usize {
    line_count.saturating_sub(page_height.max(1))
}

fn is_ignored_name(name: &str) -> bool {
    matches!(name, "target" | "node_modules" | ".git" | ".hg" | ".svn") || name.starts_with('.')
}

pub(super) fn workspace_tree_buffer(base: &std::path::Path, cap: usize) -> String {
    let mut rows = vec!["Workspace Tree".to_string(), "--------------".to_string()];
    collect_workspace_tree_rows(base, "", 0, cap, &mut rows);
    rows.push(String::new());
    rows.join("\n")
}

fn collect_workspace_tree_rows(
    base: &std::path::Path,
    relative_dir: &str,
    depth: usize,
    cap: usize,
    out: &mut Vec<String>,
) {
    if out.len() >= cap {
        return;
    }
    let dir = if relative_dir.is_empty() {
        base.to_path_buf()
    } else {
        base.join(relative_dir)
    };
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_ignored_name(name) {
            continue;
        }
        let rel = if relative_dir.is_empty() {
            name.to_string()
        } else {
            format!("{relative_dir}/{name}")
        };
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(rel);
        } else {
            files.push(rel);
        }
    }
    dirs.sort();
    files.sort();

    for rel in dirs {
        if out.len() >= cap {
            return;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        out.push(format!("{}+ {name}/  {}", "  ".repeat(depth), rel));
        collect_workspace_tree_rows(base, &rel, depth + 1, cap, out);
    }
    for rel in files {
        if out.len() >= cap {
            return;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        out.push(format!("{}- {name}  {}", "  ".repeat(depth), rel));
    }
}

pub(super) fn tree_row_path(row: &str) -> Option<&str> {
    if !row.trim_start().starts_with("- ") {
        return None;
    }
    let (_, path) = row.rsplit_once("  ")?;
    let path = path.trim();
    (!path.is_empty()).then_some(path)
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

pub(super) fn emit_cursor_moved(ctx: &mut CommandContext, old: Pos) {
    let Some(view) = ctx.workspace.views.get(&ctx.view_id) else {
        return;
    };
    if view.cursor != old {
        ctx.workspace.emit(EditorEvent::CursorMoved {
            view_id: ctx.view_id,
            pos: view.cursor,
        });
    }
}

pub(super) fn trailing_whitespace_ranges(text: &str) -> Vec<(usize, usize)> {
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

fn is_word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub(super) fn leading_whitespace(line: &str) -> &str {
    let end = line
        .bytes()
        .position(|b| b != b' ' && b != b'\t')
        .unwrap_or(line.len());
    &line[..end]
}

pub(super) fn word_forward(buf: &ozone_buffer::Buffer, pos: Pos) -> Pos {
    let line_count = buf.line_count();
    let mut line = pos.line;
    let mut col = pos.col;

    loop {
        let line_text = match buf.line(line) {
            Some(t) => t,
            None => return Pos::new(line_count.saturating_sub(1), 0),
        };
        let bytes = line_text.as_bytes();

        while col < bytes.len() && is_word_char(bytes[col]) {
            col += 1;
        }
        while col < bytes.len() && !is_word_char(bytes[col]) {
            col += 1;
        }

        if col < bytes.len() {
            return Pos::new(line, col);
        }

        if line + 1 < line_count {
            line += 1;
            col = 0;
        } else {
            return Pos::new(line, bytes.len());
        }
    }
}

pub(super) fn word_backward(buf: &ozone_buffer::Buffer, pos: Pos) -> Pos {
    let mut line = pos.line;
    let mut col = pos.col;

    loop {
        let line_text = match buf.line(line) {
            Some(t) => t,
            None => return Pos::zero(),
        };
        let bytes = line_text.as_bytes();

        if col == 0 {
            if line == 0 {
                return Pos::zero();
            }
            line -= 1;
            col = buf.line_len(line);
            continue;
        }

        col -= 1;
        while col > 0 && !is_word_char(bytes[col]) {
            col -= 1;
        }
        while col > 0 && is_word_char(bytes[col - 1]) {
            col -= 1;
        }
        return Pos::new(line, col);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collect_workspace_files, is_ignored_name, trailing_whitespace_ranges, tree_row_path,
        workspace_tree_buffer,
    };

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
    fn workspace_tree_lists_dirs_and_files() {
        let base = std::env::temp_dir().join(format!("ozone_tree_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src").join("main.rs"), "x").unwrap();
        std::fs::write(base.join("Cargo.toml"), "x").unwrap();

        let tree = workspace_tree_buffer(&base, 100);
        assert!(tree.contains("+ src/  src"));
        assert!(tree.contains("- main.rs  src/main.rs"));
        assert!(tree.contains("- Cargo.toml  Cargo.toml"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tree_row_path_extracts_files_only() {
        assert_eq!(tree_row_path("- main.rs  src/main.rs"), Some("src/main.rs"));
        assert_eq!(tree_row_path("+ src/  src"), None);
        assert_eq!(tree_row_path("Workspace Tree"), None);
    }

    #[test]
    fn goto_line_jumps_to_argument() {
        use super::{CommandContext, CommandRegistry, register_defaults};
        use crate::workspace::Workspace;
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text("a\nb\nc\nd\ne");
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let mut ctx = CommandContext::new(&mut ws)
            .unwrap()
            .with_arg(Some("3".to_string()));
        assert!(reg.execute("edit.goto-line", &mut ctx));
        assert_eq!(ws.active_view().unwrap().cursor.line, 2);
        let mut ctx = CommandContext::new(&mut ws)
            .unwrap()
            .with_arg(Some("999".to_string()));
        reg.execute("edit.goto-line", &mut ctx);
        assert_eq!(ws.active_view().unwrap().cursor.line, 4);
    }

    #[test]
    fn workspace_search_prompts_without_an_argument() {
        use super::{CommandContext, CommandRegistry, register_defaults};
        use crate::ui::UiIntent;
        use crate::workspace::Workspace;

        let mut ws = Workspace::new();
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let mut ctx = CommandContext::new(&mut ws).unwrap();

        assert!(reg.execute("search.workspace", &mut ctx));
        assert!(matches!(
            ws.drain_ui_intents().as_slice(),
            [UiIntent::Input { prompt, command }]
                if prompt == "workspace search:" && command == "search.workspace"
        ));
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
        ws.buffers
            .get_mut(&buf_id)
            .unwrap()
            .insert(Pos::new(0, 0), content);
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
