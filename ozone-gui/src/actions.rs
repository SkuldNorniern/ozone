//! Command execution + autocommand glue shared by the run loop and overlays.
//!
//! These are the GUI-side wrappers around `ozone-editor` command dispatch: run
//! a command (optionally with an argument), fire pre-save and post-event
//! autocommands, and the raw text-insert path. They are deliberately free
//! functions over `&mut Workspace` (+ the registries) so any input source —
//! keymap, palette, minibuffer, picker — drives commands the same way.

use std::io::Write;
use std::process::{Command, Stdio};

use ozone_buffer::{BufferId, BufferKind};
use ozone_editor::{
    AutocommandRegistry, CommandContext, CommandRegistry, EditorEvent, NotifyLevel, Workspace,
};

use crate::overlay::minibuffer::Minibuffer;

pub(crate) fn run_cmd(
    name: &str,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) {
    if name == "file.save" {
        if let Some(buffer_id) = ws.active_view().map(|view| view.buffer_id) {
            run_pre_save_autocmds(buffer_id, ws, reg, autocmds);
        }
    } else if name == "file.save-all" {
        let ids: Vec<_> = ws.buffers.keys().copied().collect();
        for id in ids {
            run_pre_save_autocmds(id, ws, reg, autocmds);
        }
    }

    execute_command(name, ws, reg);
    dispatch_autocmds(ws, reg, autocmds);
}

fn execute_command(name: &str, ws: &mut Workspace, reg: &CommandRegistry) {
    execute_command_arg(name, None, ws, reg);
}

fn execute_command_arg(name: &str, arg: Option<String>, ws: &mut Workspace, reg: &CommandRegistry) {
    // Two shell sigils (not registry commands):
    //   `|cmd` — *filter*: pipe the buffer through cmd (stdin → stdout → buffer).
    //            For stdin/stdout tools (rustfmt, prettier, black -); works on
    //            unsaved content. Use on `buffer.pre-save`.
    //   `!cmd` — *run*: run cmd on the file/workspace (it edits files on disk),
    //            then reload the buffer. For tools that take a path or operate on
    //            the project (cargo fmt, gofmt -w). `%` expands to the file path;
    //            cwd is the file's directory. Use on `buffer.saved` (post-write).
    if let Some(cmd_line) = name.strip_prefix('|') {
        run_shell_filter(cmd_line.trim(), ws);
        return;
    }
    if let Some(cmd_line) = name.strip_prefix('!') {
        run_shell_on_file(cmd_line.trim(), ws);
        return;
    }
    if let Some(ctx) = CommandContext::new(ws) {
        let mut ctx = ctx.with_arg(arg);
        reg.execute(name, &mut ctx);
    }
    if let Some(view) = ws.active_view_mut() {
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

/// Pipe the active file buffer through a shell command (`sh -c` / `cmd /C`),
/// replacing its contents with the command's stdout on success. Non-file
/// buffers and empty command lines are ignored. Errors surface as a warning.
fn run_shell_filter(cmd_line: &str, ws: &mut Workspace) {
    if cmd_line.is_empty() {
        return;
    }
    let Some(id) = ws.active_view().map(|v| v.buffer_id) else {
        return;
    };
    let is_file = ws
        .buffers
        .get(&id)
        .is_some_and(|b| matches!(b.kind, BufferKind::File(_)));
    if !is_file {
        return;
    }
    let Some(input) = ws.buffers.get(&id).map(|b| b.text()) else {
        return;
    };

    match pipe_through_shell(cmd_line, &input) {
        Ok(output) => {
            // A filter may emit CRLF (e.g. a Windows tool); keep the buffer LF.
            ws.replace_buffer_text(id, &ozone_buffer::LineEnding::normalize(&output));
        }
        Err(e) => ws.notify(NotifyLevel::Warn, format!("{cmd_line}: {e}")),
    }
}

/// Run `cmd_line` against the active file on disk (it edits the file/project in
/// place), then reload the buffer from disk. `%` in the command expands to the
/// file path; the command runs with the file's directory as cwd so project tools
/// (e.g. `cargo fmt`) find their manifest. Non-file buffers are ignored.
fn run_shell_on_file(cmd_line: &str, ws: &mut Workspace) {
    if cmd_line.is_empty() {
        return;
    }
    let Some(id) = ws.active_view().map(|v| v.buffer_id) else {
        return;
    };
    let path = match ws.buffers.get(&id).map(|b| &b.kind) {
        Some(BufferKind::File(p)) => p.clone(),
        _ => return,
    };

    let expanded = cmd_line.replace('%', &path.to_string_lossy());
    let cwd = path.parent().map(|p| p.to_path_buf());
    match run_shell(&expanded, cwd.as_deref()) {
        Ok(()) => {
            ws.reload_buffer(id);
        }
        Err(e) => ws.notify(NotifyLevel::Warn, format!("{cmd_line}: {e}")),
    }
}

/// Run `cmd_line` in the platform shell (optionally in `cwd`), inheriting no
/// stdin and discarding output. Returns an error on spawn failure or non-zero
/// exit (first stderr line).
fn run_shell(cmd_line: &str, cwd: Option<&std::path::Path>) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd_line]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd_line]);
        c
    };
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let out = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("could not run ({e})"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("command failed")
        .to_string())
}

/// Run `cmd_line` in the platform shell, feeding `input` on stdin and returning
/// captured stdout. stdin is written from a thread to avoid a pipe deadlock.
fn pipe_through_shell(cmd_line: &str, input: &str) -> Result<String, String> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd_line]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd_line]);
        c
    };

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run ({e})"))?;

    let mut stdin = child.stdin.take().ok_or("no stdin handle")?;
    let owned = input.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(owned.as_bytes());
        // `stdin` drops here, closing the pipe so the child sees EOF.
    });

    let out = child
        .wait_with_output()
        .map_err(|e| format!("failed ({e})"))?;
    let _ = writer.join();

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr
            .lines()
            .next()
            .unwrap_or("command failed")
            .to_string());
    }
    String::from_utf8(out.stdout).map_err(|_| "non-UTF-8 output".to_string())
}

/// Handle a key while the minibuffer prompt is open. Letters arrive via
/// TextInput; this covers submit/cancel/erase. Returns whether to redraw.
pub(crate) fn handle_minibuffer_key(
    key: aurea::KeyCode,
    minibuffer: &mut Option<Minibuffer>,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) -> bool {
    use aurea::KeyCode::*;
    let Some(mb) = minibuffer.as_mut() else {
        return false;
    };
    match key {
        Escape => {
            *minibuffer = None;
            true
        }
        Enter => {
            let (cmd, input) = (mb.command.clone(), mb.input.clone());
            *minibuffer = None;
            run_cmd_with_arg(&cmd, input, ws, reg, autocmds);
            true
        }
        Backspace => {
            mb.input.pop();
            true
        }
        _ => false,
    }
}

/// Run a command with a string argument (minibuffer submit), then dispatch
/// autocommands. Mirrors `run_cmd` but passes `arg` to the command.
pub(crate) fn run_cmd_with_arg(
    name: &str,
    arg: String,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) {
    execute_command_arg(name, Some(arg), ws, reg);
    dispatch_autocmds(ws, reg, autocmds);
}

fn run_pre_save_autocmds(
    buffer_id: BufferId,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) {
    let path = ws.buffers.get(&buffer_id).and_then(|buf| match &buf.kind {
        BufferKind::File(path) => Some(path.clone()),
        _ => None,
    });
    let Some(path) = path else {
        return;
    };

    let event = EditorEvent::BufferPreSave {
        id: buffer_id,
        path,
    };
    let commands: Vec<String> = autocmds
        .matching_commands(&event)
        .into_iter()
        .map(str::to_string)
        .collect();
    for command in commands {
        if command == "file.save" || command == "file.save-all" {
            continue;
        }
        execute_command(&command, ws, reg);
    }
}

pub(crate) fn dispatch_autocmds(
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) {
    const MAX_AUTOCMD_ROUNDS: usize = 16;

    for _ in 0..MAX_AUTOCMD_ROUNDS {
        let events = ws.drain_events();
        if events.is_empty() {
            break;
        }

        let commands: Vec<String> = events
            .iter()
            .flat_map(|event| autocmds.matching_commands(event))
            .map(str::to_string)
            .collect();

        if commands.is_empty() {
            continue;
        }

        for command in commands {
            if command == "file.save" || command == "file.save-all" {
                continue;
            }
            execute_command(&command, ws, reg);
        }
    }
}

pub(crate) fn insert_text_raw(text: &str, ws: &mut Workspace) -> bool {
    let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
    if filtered.is_empty() {
        return false;
    }

    let Some(view) = ws.active_view() else {
        return false;
    };
    let cursor = view.cursor;
    let buf_id = view.buffer_id;

    // Virtual/read-only surfaces (pickers, terminal placeholder) reject edits.
    if matches!(
        ws.buffers.get(&buf_id).map(|b| &b.kind),
        Some(
            BufferKind::Search
                | BufferKind::References
                | BufferKind::FileTree
                | BufferKind::Terminal
                | BufferKind::Image(_)
        )
    ) {
        return false;
    }

    if let Some(buf) = ws.buffers.get_mut(&buf_id) {
        let delta = buf.insert(cursor, &filtered);
        // Cursor columns are byte offsets (see Pos docs); advance by the inserted
        // byte length, not the char count, or multi-byte input desyncs the cursor
        // from the buffer offset.
        let bytes = filtered.len();
        let cursor_event = ws.active_view_mut().map(|view| {
            view.cursor.col += bytes;
            view.col_memory = view.cursor.col;
            view.scroll_to_cursor(view.page_height.max(1));
            EditorEvent::CursorMoved {
                view_id: view.id,
                pos: view.cursor,
            }
        });
        if let Some(event) = cursor_event {
            ws.emit(event);
        }
        ws.emit(EditorEvent::BufferChanged { id: buf_id, delta });
        return true;
    }
    false
}
