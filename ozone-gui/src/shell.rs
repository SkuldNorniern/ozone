//! Non-blocking shell execution for `!cmd` / `|cmd` autocommands.
//!
//! Each invocation spawns the user's shell on a background thread; the editor
//! keeps running while it executes. [`ShellJobs::poll`] (called once per
//! frame) collects finished jobs, applies their effect to the workspace
//! (replace buffer text / reload from disk), and surfaces stdout/stderr via
//! notifications.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use ozone_buffer::BufferId;
use ozone_editor::{
    NotifyLevel, Workspace, WorkspaceMatch,
    workspace_search::{MAX_SEARCH_FILES, MAX_SEARCH_RESULTS, search_workspace_with_progress},
};

/// What to do with a job's output once it finishes.
enum JobKind {
    /// `|cmd` — replace the buffer's text with stdout on success.
    Filter(BufferId),
    /// `!cmd` — reload the buffer from disk on success (the command edited it).
    Run(BufferId),
}

/// Captured output of a finished shell command.
struct ShellOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

struct Job {
    cmd_line: String,
    kind: JobKind,
    rx: Receiver<ShellOutput>,
}

/// Tracks shell commands spawned by autocommands until they finish.
#[derive(Default)]
pub(crate) struct ShellJobs {
    jobs: Vec<Job>,
}

impl ShellJobs {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `|cmd` — pipe `input` (the buffer's current text) through `cmd_line` on
    /// a background thread; on success its stdout replaces the buffer text.
    pub(crate) fn spawn_filter(&mut self, cmd_line: &str, buffer_id: BufferId, input: String) {
        self.jobs.push(Job {
            cmd_line: cmd_line.to_string(),
            kind: JobKind::Filter(buffer_id),
            rx: spawn(cmd_line, Some(input)),
        });
    }

    /// `!cmd` — run `cmd_line` on a background thread; on success the buffer
    /// is reloaded from disk (the command edited the file/project in place).
    pub(crate) fn spawn_run(&mut self, cmd_line: &str, buffer_id: BufferId) {
        self.jobs.push(Job {
            cmd_line: cmd_line.to_string(),
            kind: JobKind::Run(buffer_id),
            rx: spawn(cmd_line, None),
        });
    }

    /// Apply finished jobs to `ws` and notify of their output. Returns
    /// whether any job finished (a redraw is warranted).
    pub(crate) fn poll(&mut self, ws: &mut Workspace) -> bool {
        let mut changed = false;
        self.jobs.retain(|job| match job.rx.try_recv() {
            Ok(out) => {
                changed = true;
                apply(job, out, ws);
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        changed
    }
}

fn apply(job: &Job, out: ShellOutput, ws: &mut Workspace) {
    if !out.success {
        ws.notify(NotifyLevel::Warn, format_output(&job.cmd_line, &out));
        return;
    }
    match job.kind {
        JobKind::Filter(id) => {
            // A filter may emit CRLF (e.g. a Windows tool); keep the buffer LF.
            ws.replace_buffer_text(id, &ozone_buffer::LineEnding::normalize(&out.stdout));
        }
        JobKind::Run(id) => {
            ws.reload_buffer(id);
            if !out.stdout.trim().is_empty() || !out.stderr.trim().is_empty() {
                ws.notify(NotifyLevel::Info, format_output(&job.cmd_line, &out));
            }
        }
    }
}

/// Combine a job's stdout/stderr into one notification line.
fn format_output(cmd_line: &str, out: &ShellOutput) -> String {
    let mut text = String::new();
    if !out.stdout.trim().is_empty() {
        text.push_str(out.stdout.trim());
    }
    if !out.stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push_str(" | ");
        }
        text.push_str(out.stderr.trim());
    }
    if text.is_empty() {
        text.push_str(if out.success {
            "(no output)"
        } else {
            "command failed"
        });
    }
    format!("{cmd_line}: {text}")
}

/// Spawn `cmd_line` in the user's shell on a background thread, optionally
/// feeding `input` on stdin, and return a channel that receives its output
/// once it exits. The command runs in the current workspace directory.
fn spawn(cmd_line: &str, input: Option<String>) -> Receiver<ShellOutput> {
    let (tx, rx) = channel();
    let cmd_line = cmd_line.to_string();
    let cwd = std::env::current_dir().ok();
    std::thread::spawn(move || {
        let _ = tx.send(run(&cmd_line, cwd.as_deref(), input));
    });
    rx
}

fn run(cmd_line: &str, cwd: Option<&std::path::Path>, input: Option<String>) -> ShellOutput {
    let mut command = shell_command(cmd_line);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Without this, spawning cmd.exe flashes a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ShellOutput {
                success: false,
                stdout: String::new(),
                stderr: format!("could not run ({e})"),
            };
        }
    };

    // Write stdin from a separate thread so a child that writes a lot of
    // stdout before reading all of stdin can't deadlock us.
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }

    match child.wait_with_output() {
        Ok(out) => ShellOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => ShellOutput {
            success: false,
            stdout: String::new(),
            stderr: format!("failed ({e})"),
        },
    }
}

/// Background workspace-wide literal search job. Spawns a thread that runs
/// `search_workspace_with_progress` and sends total file count, per-file
/// progress, and final results back via mpsc so the UI can show a live bar.
pub(crate) struct WorkspaceSearchJob {
    pub(crate) query: String,
    /// Id of the "Searching…" notification so the poller can dismiss it.
    pub(crate) notif_id: u64,
    rx: Receiver<WorkspaceSearchMsg>,
    pub(crate) files_scanned: usize,
    /// Total files to scan; 0 until the thread sends `TotalFiles`.
    pub(crate) total_files: usize,
}

enum WorkspaceSearchMsg {
    TotalFiles(usize),
    Progress(usize),
    Done(Vec<WorkspaceMatch>),
}

impl WorkspaceSearchJob {
    pub(crate) fn spawn(base: PathBuf, query: String, notif_id: u64) -> Self {
        let (tx, rx) = channel();
        let q = query.clone();
        let total_tx = tx.clone();
        let progress_tx = tx.clone();
        std::thread::spawn(move || {
            let matches = search_workspace_with_progress(
                &base,
                &q,
                MAX_SEARCH_FILES,
                MAX_SEARCH_RESULTS,
                move |total| {
                    let _ = total_tx.send(WorkspaceSearchMsg::TotalFiles(total));
                },
                |n| {
                    if n % 100 == 0 {
                        let _ = progress_tx.send(WorkspaceSearchMsg::Progress(n));
                    }
                },
            );
            let _ = tx.send(WorkspaceSearchMsg::Done(matches));
        });
        Self {
            query,
            notif_id,
            rx,
            files_scanned: 0,
            total_files: 0,
        }
    }

    /// Non-blocking poll. Returns `Some(matches)` when the search is done.
    pub(crate) fn poll(&mut self) -> Option<Vec<WorkspaceMatch>> {
        loop {
            match self.rx.try_recv() {
                Ok(WorkspaceSearchMsg::TotalFiles(n)) => self.total_files = n,
                Ok(WorkspaceSearchMsg::Progress(n)) => {
                    self.files_scanned = n;
                }
                Ok(WorkspaceSearchMsg::Done(matches)) => return Some(matches),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => return Some(Vec::new()),
            }
        }
    }
}

/// Build the user's shell invocation for `cmd_line`: `%COMSPEC% /C` on
/// Windows, `$SHELL -c` elsewhere (falling back to `cmd` / `sh`).
fn shell_command(cmd_line: &str) -> Command {
    if cfg!(windows) {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".to_string());
        let mut c = Command::new(shell);
        c.args(["/C", cmd_line]);
        c
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let mut c = Command::new(shell);
        c.args(["-c", cmd_line]);
        c
    }
}
