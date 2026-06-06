//! Process-backed terminal core — std only, no external crates.
//!
//! Spawns a shell with piped stdio, drains stdout+stderr on reader threads into
//! a shared, ANSI-stripped output buffer, and writes user input to stdin. This
//! is a "dumb" pipe terminal: it runs ordinary commands (dir, cargo, git) well;
//! full interactive TUIs need a PTY (ConPTY) + VT parser, a planned upgrade that
//! can slot behind this same API.

pub mod ansi;

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

/// Max retained output bytes; older output is trimmed from the front.
const MAX_OUTPUT: usize = 256 * 1024;

/// A running shell process with captured, ANSI-stripped output.
pub struct Terminal {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Arc<Mutex<String>>,
}

impl Terminal {
    /// Spawn the platform default shell with piped stdio.
    pub fn spawn() -> std::io::Result<Self> {
        Self::spawn_program(default_shell())
    }

    /// Spawn a specific program (with args) as the terminal process.
    pub fn spawn_program((program, args): (String, Vec<String>)) -> std::io::Result<Self> {
        let mut child = Command::new(&program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let output = Arc::new(Mutex::new(String::new()));
        if let Some(out) = child.stdout.take() {
            spawn_reader(out, output.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_reader(err, output.clone());
        }
        let stdin = child.stdin.take();
        Ok(Self { child, stdin, output })
    }

    /// Send a line (a newline is appended) to the shell.
    pub fn write_line(&mut self, line: &str) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.write_all(b"\n");
            let _ = stdin.flush();
        }
    }

    /// Write raw bytes (no newline) to the shell.
    pub fn write_str(&mut self, s: &str) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(s.as_bytes());
            let _ = stdin.flush();
        }
    }

    /// A copy of the current accumulated (ANSI-stripped) output.
    pub fn output_snapshot(&self) -> String {
        self.output.lock().map(|o| o.clone()).unwrap_or_default()
    }

    /// Total bytes of captured output (cheap; for change detection).
    pub fn output_len(&self) -> usize {
        self.output.lock().map(|o| o.len()).unwrap_or(0)
    }

    /// Whether the shell process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn default_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        // /Q disables command echo so we can echo input locally without doubling.
        ("cmd.exe".to_string(), vec!["/Q".to_string()])
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (shell, vec!["-i".to_string()])
    }
}

fn spawn_reader<R: Read + Send + 'static>(mut reader: R, output: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    let cleaned = ansi::strip(&text);
                    if let Ok(mut o) = output.lock() {
                        o.push_str(&cleaned);
                        if o.len() > MAX_OUTPUT {
                            let cut = o.len() - MAX_OUTPUT;
                            // trim to a char boundary
                            let mut cut = cut;
                            while cut < o.len() && !o.is_char_boundary(cut) {
                                cut += 1;
                            }
                            *o = o.split_off(cut);
                        }
                    }
                }
            }
        }
    });
}
