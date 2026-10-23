//! PTY-backed terminal core with a small VT emulator.
//!
//! [`Terminal`] spawns a shell on a pseudo-terminal (so interactive programs
//! work: prompts, line editing, `^C`, `wsl`, ssh) and feeds its output through a
//! [`vt::Vt`] grid that interprets cursor/erase escapes — so layout is correct
//! instead of mangled by naive escape stripping.
//!
//! The PTY backend is swappable behind [`pty::Pty`]:
//! - `portable` (default): the `portable-pty` crate.
//! - `inhouse`: direct ConPTY / forkpty FFI (scaffold — see `pty::inhouse`).

pub mod pty;
pub mod vt;

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use vt::Vt;
pub use vt::{Cell, Color};

/// Initial PTY/grid size (cells). The GUI resizes to the visible area.
const INIT_COLS: u16 = 120;
const INIT_ROWS: u16 = 40;

/// A slice of the terminal's lines, from [`Terminal::cell_window`].
#[derive(Clone, Debug, Default)]
pub struct Window {
    /// Lines in scrollback plus screen.
    pub lines: usize,
    /// The requested lines.
    pub rows: Vec<Vec<Cell>>,
    /// The cursor's absolute `(line, col)`.
    pub cursor: (usize, usize),
}

/// A running shell on a PTY, with its output rendered through a VT grid.
pub struct Terminal {
    pty: Box<dyn pty::Pty>,
    vt: Arc<Mutex<Vt>>,
    /// Bumped by the reader thread on every processed chunk; lets callers skip
    /// re-rendering the whole grid when nothing new has arrived.
    version: Arc<AtomicU64>,
}

impl Terminal {
    /// Spawn the platform default shell on a PTY using the configured backend.
    pub fn spawn() -> std::io::Result<Self> {
        Self::spawn_in(None, || {})
    }

    /// Spawn the shell in `cwd` (the current directory when `None`).
    /// `on_output` runs on the reader thread after each processed chunk, so a
    /// GUI can wake up and redraw instead of polling.
    pub fn spawn_in(
        cwd: Option<&Path>,
        on_output: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let mut pty = pty::spawn_default(cwd)?;
        let reader = pty
            .take_reader()
            .ok_or_else(|| std::io::Error::other("pty produced no reader"))?;
        pty.resize(INIT_COLS, INIT_ROWS);

        let vt = Arc::new(Mutex::new(Vt::new(INIT_COLS as usize, INIT_ROWS as usize)));
        let version = Arc::new(AtomicU64::new(0));
        spawn_reader(reader, vt.clone(), version.clone(), on_output);
        Ok(Self { pty, vt, version })
    }

    /// A monotonically increasing counter of processed output chunks. Unchanged
    /// since a previous read means there is nothing new to render.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Write raw bytes to the PTY input (the shell's line discipline echoes them).
    pub fn write_str(&self, s: &str) {
        let _ = self.pty.write(s.as_bytes());
    }

    /// Resize the PTY and the VT grid to `cols` x `rows` character cells.
    pub fn resize(&self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if let Ok(mut v) = self.vt.lock() {
            v.resize(cols as usize, rows as usize);
        }
        self.pty.resize(cols, rows);
        // Force the next snapshot: the grid reflowed even if no new bytes arrived.
        self.version.fetch_add(1, Ordering::Release);
    }

    /// The current screen (scrollback + grid) rendered as text.
    pub fn output_snapshot(&self) -> String {
        self.vt.lock().map(|v| v.render()).unwrap_or_default()
    }

    /// The current screen as coloured cells (scrollback + grid), one row per line.
    pub fn cell_snapshot(&self) -> Vec<Vec<Cell>> {
        self.vt.lock().map(|v| v.render_cells()).unwrap_or_default()
    }

    /// Total lines, up to `count` lines from `first`, and the cursor, all read
    /// under one lock so they agree with each other.
    pub fn cell_window(&self, first: usize, count: usize) -> Window {
        self.vt
            .lock()
            .map(|v| Window {
                lines: v.line_count(),
                rows: v.render_window(first, count),
                cursor: v.cursor(),
            })
            .unwrap_or_default()
    }

    /// The cursor's absolute `(line, col)` in the rendered output.
    pub fn cursor(&self) -> (usize, usize) {
        self.vt.lock().map(|v| v.cursor()).unwrap_or((0, 0))
    }

    /// Whether the shell process is still running.
    pub fn is_running(&self) -> bool {
        self.pty.is_running()
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    vt: Arc<Mutex<Vt>>,
    version: Arc<AtomicU64>,
    on_output: impl Fn() + Send + 'static,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        // Bytes of a char split across two reads wait here for the rest.
        let mut carry: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    carry.extend_from_slice(&buf[..n]);
                    let text = take_text(&mut carry);
                    if let Ok(mut v) = vt.lock() {
                        v.process(&text);
                    }
                    version.fetch_add(1, Ordering::Release);
                    on_output();
                }
            }
        }
    });
}

/// Decodes what `bytes` holds as text, leaving an incomplete trailing char
/// in `bytes` for the next read. Invalid bytes become U+FFFD.
fn take_text(bytes: &mut Vec<u8>) -> String {
    let mut out = String::new();
    let mut rest: &[u8] = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                rest = &[];
                break;
            }
            Err(e) => {
                let (good, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap_or_default());
                match e.error_len() {
                    Some(bad) => {
                        out.push('\u{fffd}');
                        rest = &after[bad..];
                    }
                    // Cut short at the end: keep it for the next read.
                    None => {
                        rest = after;
                        break;
                    }
                }
            }
        }
    }
    let start = bytes.len() - rest.len();
    bytes.drain(..start);
    out
}

#[cfg(test)]
mod tests {
    use super::take_text;

    #[test]
    fn split_chars_wait_for_the_next_read() {
        let hangul = "가".as_bytes();
        let mut carry = vec![b'a', hangul[0], hangul[1]];
        assert_eq!(take_text(&mut carry), "a");
        assert_eq!(carry.len(), 2);
        carry.push(hangul[2]);
        assert_eq!(take_text(&mut carry), "가");
        assert!(carry.is_empty());
    }

    #[test]
    fn bad_bytes_become_replacement() {
        let mut carry = vec![b'x', 0xff, b'y'];
        assert_eq!(take_text(&mut carry), "x\u{fffd}y");
        assert!(carry.is_empty());
    }
}
