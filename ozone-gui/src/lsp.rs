//! GUI-side LSP orchestration: lazily start the configured server, mirror open
//! Rust buffers to it (`didOpen`/`didChange`/`didClose`), and route incoming
//! diagnostics into the decoration store via `Workspace::publish_diagnostics`.
//!
//! The editor core stays transport-free; this module is the single place the
//! frontend drives the `ozone_lsp` client. `sync` is called once per frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ozone_buffer::{Buffer, BufferId, BufferKind, Pos};
use ozone_config::Config;
use ozone_editor::{Diagnostic, NamespaceId, NotifyLevel, Workspace};
use ozone_lsp::{CompletionItem, Location, LspClient, ServerMessage};
use taste::{Language, detect_language};

/// Coalesce rapid edits: send at most one `didChange` per document per window.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(150);

/// One mirrored document: its file URI, the canonical path (for matching server
/// diagnostics back to a buffer), the last synced buffer revision, the LSP
/// version, and when we last sent a change (for debouncing).
struct Doc {
    uri: String,
    canonical: PathBuf,
    revision: u64,
    version: i64,
    last_change: Instant,
}

/// Coarse server lifecycle for the status-bar indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LspStatus {
    #[default]
    Idle,
    Starting,
    Ready,
    Failed,
}

/// Frontend LSP state. Holds at most one server for now (Rust); generalizing to
/// a server-per-language map is a later step.
#[derive(Default)]
pub(crate) struct Lsp {
    client: Option<LspClient>,
    /// Background thread starting the server; polled non-blocking each frame so
    /// the GUI stays live during the initialize handshake.
    starting: Option<Receiver<Result<LspClient, String>>>,
    /// True once a start attempt failed, so we don't respawn on every frame.
    failed: bool,
    /// Diagnostics decoration namespace, allocated on first start.
    namespace: Option<NamespaceId>,
    docs: HashMap<BufferId, Doc>,
    /// In-flight `textDocument/definition` request id, if any.
    pending_goto: Option<i64>,
    /// In-flight `textDocument/hover` request id, if any.
    pending_hover: Option<i64>,
    /// In-flight `textDocument/completion` request, if any: id, the buffer it
    /// was sent for, and where the completed prefix starts (the range the
    /// chosen item replaces on commit).
    pending_completion: Option<(i64, BufferId, Pos)>,
    /// A completion result ready for the frontend to show as a popup, taken
    /// via [`Lsp::take_completion_result`].
    completion_result: Option<CompletionResult>,
}

/// A `textDocument/completion` result, ready to show as a popup.
pub(crate) struct CompletionResult {
    pub(crate) items: Vec<CompletionItem>,
    pub(crate) buffer_id: BufferId,
    /// Start of the identifier prefix being completed; the popup replaces
    /// `[anchor, cursor)` with the chosen item's `insert_text`.
    pub(crate) anchor: Pos,
}

impl Lsp {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn status(&self) -> LspStatus {
        if self.failed {
            LspStatus::Failed
        } else if self.client.is_some() {
            LspStatus::Ready
        } else if self.starting.is_some() {
            LspStatus::Starting
        } else {
            LspStatus::Idle
        }
    }

    /// Reconcile the server with the current set of open Rust buffers and drain
    /// any diagnostics. Returns whether a redraw is warranted.
    pub(crate) fn sync(&mut self, ws: &mut Workspace, config: &Config) -> bool {
        if self.failed {
            return false;
        }
        let Some(cfg) = config.lsps.iter().find(|l| l.language == "rust") else {
            return false; // no Rust server configured
        };

        let open: Vec<(BufferId, PathBuf)> = rust_file_buffers(ws);

        // Lazy start: only once a Rust file is actually open. The handshake
        // runs on a background thread so the GUI stays live; we poll the
        // receiver non-blocking each frame until it resolves.
        if self.client.is_none() {
            if let Some(rx) = &self.starting {
                use std::sync::mpsc::TryRecvError;
                match rx.try_recv() {
                    Ok(Ok(client)) => {
                        self.client = Some(client);
                        self.namespace = Some(ws.decorations_mut().namespace());
                        self.starting = None;
                        // Fall through to open_and_update below.
                    }
                    Ok(Err(e)) => {
                        self.failed = true;
                        self.starting = None;
                        ws.notify(NotifyLevel::Warn, format!("LSP: {e}"));
                        return true;
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.failed = true;
                        self.starting = None;
                        ws.notify(NotifyLevel::Warn, "LSP: server thread panicked".to_string());
                        return true;
                    }
                    Err(TryRecvError::Empty) => return false, // still starting
                }
            } else {
                if open.is_empty() {
                    return false;
                }
                let server = cfg.server.clone();
                let args = cfg.args.clone();
                let root_uri =
                    path_to_uri(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(LspClient::start(&server, &args, &root_uri));
                });
                self.starting = Some(rx);
                return false;
            }
        }

        let mut changed = false;
        changed |= self.open_and_update(ws, &open);
        changed |= self.close_gone(ws, &open);
        changed |= self.drain_messages(ws);
        changed
    }

    /// Send `textDocument/definition` for the active buffer's cursor position.
    /// Requires that the buffer is already mirrored (didOpen sent). No-op if
    /// the server isn't running or the buffer isn't a mirrored Rust file.
    pub(crate) fn request_goto_definition(&mut self, ws: &Workspace) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if !client.capabilities.definition {
            return;
        }
        let Some(view) = ws.active_view() else {
            return;
        };
        let buf_id = view.buffer_id;
        let cursor = view.cursor;
        let Some(doc) = self.docs.get(&buf_id) else {
            return; // buffer not yet mirrored — server doesn't know about it
        };
        let utf16_col = ws
            .buffers
            .get(&buf_id)
            .and_then(|b| b.line(cursor.line))
            .map(|line| byte_to_utf16_col(&line, cursor.col))
            .unwrap_or(cursor.col);
        let uri = doc.uri.clone();
        let id = client.goto_definition(&uri, cursor.line as u32, utf16_col as u32);
        self.pending_goto = Some(id);
    }

    /// Send `textDocument/hover` for the active buffer's cursor position.
    /// Shows the result as a notification. No-op conditions same as `request_goto_definition`.
    pub(crate) fn request_hover(&mut self, ws: &mut Workspace) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if !client.capabilities.hover {
            return;
        }
        let Some(view) = ws.active_view() else {
            return;
        };
        let buf_id = view.buffer_id;
        let cursor = view.cursor;
        let Some(doc) = self.docs.get(&buf_id) else {
            return;
        };
        let utf16_col = ws
            .buffers
            .get(&buf_id)
            .and_then(|b| b.line(cursor.line))
            .map(|line| byte_to_utf16_col(&line, cursor.col))
            .unwrap_or(cursor.col);
        let uri = doc.uri.clone();
        let id = client.hover(&uri, cursor.line as u32, utf16_col as u32);
        self.pending_hover = Some(id);
    }

    /// Send `textDocument/completion` for the active buffer's cursor position.
    /// The result is picked up via [`Lsp::take_completion_result`] once the
    /// server responds. No-op conditions same as `request_goto_definition`.
    pub(crate) fn request_completion(&mut self, ws: &Workspace) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if !client.capabilities.completion {
            return;
        }
        let Some(view) = ws.active_view() else {
            return;
        };
        let buf_id = view.buffer_id;
        let cursor = view.cursor;
        let Some(doc) = self.docs.get(&buf_id) else {
            return;
        };
        let Some(line) = ws.buffers.get(&buf_id).and_then(|b| b.line(cursor.line)) else {
            return;
        };
        let anchor = Pos::new(cursor.line, word_start(&line, cursor.col));
        let utf16_col = byte_to_utf16_col(&line, cursor.col);
        let uri = doc.uri.clone();
        let id = client.completion(&uri, cursor.line as u32, utf16_col as u32);
        self.pending_completion = Some((id, buf_id, anchor));
    }

    /// Take a completion result if one has arrived since the last call.
    pub(crate) fn take_completion_result(&mut self) -> Option<CompletionResult> {
        self.completion_result.take()
    }

    /// `didOpen` new Rust buffers and `didChange` ones whose revision advanced
    /// (cheap O(1) check; full text sent only when we actually fire a change).
    fn open_and_update(&mut self, ws: &mut Workspace, open: &[(BufferId, PathBuf)]) -> bool {
        let Some(client) = self.client.as_mut() else {
            return false;
        };
        for (id, path) in open {
            let Some(revision) = ws.buffers.get(id).map(Buffer::revision) else {
                continue;
            };
            match self.docs.get_mut(id) {
                None => {
                    let text = ws.buffers.get(id).map(Buffer::text).unwrap_or_default();
                    let uri = path_to_uri(path);
                    client.did_open(&uri, "rust", 1, &text);
                    self.docs.insert(
                        *id,
                        Doc {
                            uri,
                            canonical: canonicalize(path),
                            revision,
                            version: 1,
                            last_change: Instant::now(),
                        },
                    );
                }
                // Revision advanced — sync, but no more than once per debounce
                // window. Deferred frames keep the stale revision and retry.
                Some(doc) if doc.revision != revision => {
                    if doc.last_change.elapsed() >= CHANGE_DEBOUNCE {
                        let text = ws.buffers.get(id).map(Buffer::text).unwrap_or_default();
                        doc.version += 1;
                        doc.revision = revision;
                        doc.last_change = Instant::now();
                        client.did_change(&doc.uri, doc.version, &text);
                    }
                }
                Some(_) => {}
            }
        }
        false // notifications alone don't need a redraw
    }

    /// `didClose` + clear diagnostics for buffers that are no longer open.
    fn close_gone(&mut self, ws: &mut Workspace, open: &[(BufferId, PathBuf)]) -> bool {
        let gone: Vec<BufferId> = self
            .docs
            .keys()
            .copied()
            .filter(|id| !open.iter().any(|(open_id, _)| open_id == id))
            .collect();
        if gone.is_empty() {
            return false;
        }
        let ns = self.namespace.unwrap_or(0);
        for id in gone {
            if let Some(doc) = self.docs.remove(&id) {
                if let Some(client) = self.client.as_mut() {
                    client.did_close(&doc.uri);
                }
                // Buffer may already be gone; publish_diagnostics no-ops then.
                ws.publish_diagnostics(id, ns, &[]);
            }
        }
        true
    }

    /// Drain all server messages: route diagnostics to the decoration store and
    /// goto-definition results to a cursor jump.
    fn drain_messages(&mut self, ws: &mut Workspace) -> bool {
        let Some(client) = self.client.as_ref() else {
            return false;
        };
        let messages = client.poll();
        if messages.is_empty() {
            return false;
        }
        let ns = self.namespace.unwrap_or(0);
        let mut changed = false;
        for msg in messages {
            match msg {
                ServerMessage::Diagnostics { uri, diagnostics } => {
                    let Some(target) = uri_to_path(&uri).map(|p| canonicalize(&p)) else {
                        continue;
                    };
                    let Some(id) = self
                        .docs
                        .iter()
                        .find(|(_, d)| d.canonical == target)
                        .map(|(id, _)| *id)
                    else {
                        continue;
                    };
                    let remapped = remap_to_byte_cols(ws, id, diagnostics);
                    ws.publish_diagnostics(id, ns, &remapped);
                    changed = true;
                }
                ServerMessage::GotoDefinitionResult { id, locations } => {
                    if self.pending_goto == Some(id) {
                        self.pending_goto = None;
                        if let Some(loc) = locations.into_iter().next() {
                            jump_to_location(ws, loc);
                            changed = true;
                        }
                    }
                }
                ServerMessage::HoverResult { id, contents } => {
                    if self.pending_hover == Some(id) {
                        self.pending_hover = None;
                        if let Some(text) = contents {
                            ws.notify(NotifyLevel::Info, text);
                            changed = true;
                        }
                    }
                }
                ServerMessage::CompletionResult { id, items } => {
                    if let Some((pending_id, buffer_id, anchor)) = self.pending_completion
                        && pending_id == id
                    {
                        self.pending_completion = None;
                        if !items.is_empty() {
                            self.completion_result = Some(CompletionResult {
                                items,
                                buffer_id,
                                anchor,
                            });
                            changed = true;
                        }
                    }
                }
            }
        }
        changed
    }
}

/// Remap each diagnostic's `character` (a UTF-16 code-unit offset, per the LSP
/// spec) to a byte column using the buffer's line text. Correct for non-ASCII
/// lines; a no-op for ASCII. Unknown lines are left unchanged.
fn remap_to_byte_cols(ws: &Workspace, id: BufferId, diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let Some(buf) = ws.buffers.get(&id) else {
        return diags;
    };
    diags
        .into_iter()
        .map(|d| Diagnostic {
            start: byte_pos(buf, d.start),
            end: byte_pos(buf, d.end),
            ..d
        })
        .collect()
}

/// Convert a `(line, utf16_col)` position to `(line, byte_col)`.
fn byte_pos(buf: &Buffer, pos: Pos) -> Pos {
    match buf.line(pos.line) {
        Some(line) => Pos::new(pos.line, utf16_to_byte_col(&line, pos.col)),
        None => pos,
    }
}

/// Byte offset of the `utf16`-th UTF-16 code unit within `line`. Clamps to the
/// line length if the offset runs past the end.
fn utf16_to_byte_col(line: &str, utf16: usize) -> usize {
    if utf16 == 0 {
        return 0;
    }
    let mut units = 0;
    for (byte_idx, ch) in line.char_indices() {
        if units >= utf16 {
            return byte_idx;
        }
        units += ch.len_utf16();
    }
    line.len()
}

/// Byte column of the `byte_col`-th byte within `line`, expressed as a UTF-16
/// code-unit offset. Inverse of [`utf16_to_byte_col`]; used before sending a
/// cursor position to the server.
fn byte_to_utf16_col(line: &str, byte_col: usize) -> usize {
    line[..byte_col.min(line.len())]
        .chars()
        .map(|c| c.len_utf16())
        .sum()
}

/// Byte offset where the identifier ending at `col` on `line` starts: scan
/// backward over `[A-Za-z0-9_]` bytes. Returns `col` unchanged if the cursor
/// isn't preceded by an identifier character (the completion then inserts at
/// the cursor with nothing to replace).
fn word_start(line: &str, col: usize) -> usize {
    let bytes = line.as_bytes();
    let mut start = col.min(bytes.len());
    while start > 0 && matches!(bytes[start - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_') {
        start -= 1;
    }
    start
}

/// Jump the workspace to an LSP [`Location`]: switch to the target file (or
/// open it if not already loaded), move the cursor, push a jump-list entry.
fn jump_to_location(ws: &mut Workspace, loc: Location) {
    let Some(path) = uri_to_path(&loc.uri) else {
        return;
    };
    let canonical = canonicalize(&path);

    // Reuse an already-open buffer for this file when possible.
    let existing_id = ws.buffers.iter().find_map(|(id, buf)| {
        let buf_path = match &buf.kind {
            BufferKind::File(p) => p.clone(),
            _ => return None,
        };
        if canonicalize(&buf_path) == canonical {
            Some(*id)
        } else {
            None
        }
    });

    // (Both switch_active_buffer and open_file push a jump internally.)
    let view_id = if let Some(buf_id) = existing_id {
        ws.switch_active_buffer(buf_id);
        ws.active_view_id
    } else {
        match ws.open_file(path) {
            Ok((_, vid)) => Some(vid),
            Err(_) => return,
        }
    };

    let Some(view_id) = view_id else { return };
    let Some(buf_id) = ws.views.get(&view_id).map(|v| v.buffer_id) else {
        return;
    };
    let (line, col) = {
        let Some(buf) = ws.buffers.get(&buf_id) else {
            return;
        };
        let line = loc.line.min(buf.line_count().saturating_sub(1));
        let byte_col = buf
            .line(line)
            .map(|l| utf16_to_byte_col(&l, loc.character))
            .unwrap_or(0);
        (line, byte_col)
    };
    if let Some(view) = ws.views.get_mut(&view_id) {
        view.cursor = Pos::new(line, col);
        view.col_memory = col;
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

/// Open file buffers whose filetype is Rust.
fn rust_file_buffers(ws: &Workspace) -> Vec<(BufferId, PathBuf)> {
    ws.buffers
        .iter()
        .filter_map(|(id, b)| match &b.kind {
            BufferKind::File(p)
                if detect_language(p.as_os_str().to_str().unwrap_or(""))
                    == Some(Language::RUST) =>
            {
                Some((*id, p.clone()))
            }
            _ => None,
        })
        .collect()
}

fn canonicalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Convert an absolute path to a `file://` URI. Minimal percent-encoding (space)
/// is enough for the server to accept it; diagnostics are matched back by
/// canonical path, not by URI string equality.
fn path_to_uri(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    // Windows absolute paths (`C:/…`) need a leading slash after `file://`.
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    let encoded = s.replace(' ', "%20");
    format!("file://{encoded}")
}

/// Convert a `file://` URI back to a path. Handles the Windows `/C:/…` form and
/// percent-decoding. Returns `None` for non-file URIs.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Drop an authority component if present (`file://host/path`); local files
    // use an empty authority so `rest` already starts with `/`.
    let path = rest.strip_prefix('/').unwrap_or(rest);
    let decoded = percent_decode(path);
    // `/C:/foo` → `C:/foo` on Windows; POSIX keeps its leading slash.
    let normalized = if cfg!(windows) {
        decoded
    } else {
        format!("/{decoded}")
    };
    Some(PathBuf::from(normalized))
}

/// Decode `%XX` escapes in a URI path component.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_path_roundtrip() {
        let uri = path_to_uri(Path::new("/home/u/My Code/main.rs"));
        assert!(uri.starts_with("file:///"));
        assert!(uri.contains("%20"));
        let back = uri_to_path(&uri).unwrap();
        assert!(back.to_string_lossy().contains("My Code"));
        assert!(back.to_string_lossy().ends_with("main.rs"));
    }

    #[test]
    fn percent_decode_handles_escapes_and_literals() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("100%done"), "100%done"); // malformed escape kept
    }

    #[test]
    fn uri_to_path_rejects_non_file() {
        assert!(uri_to_path("http://example.com").is_none());
    }

    #[test]
    fn word_start_finds_identifier_prefix() {
        assert_eq!(word_start("    let hash_ma", 16), 8);
        assert_eq!(word_start("foo.", 4), 4); // not preceded by an identifier char
        assert_eq!(word_start("", 0), 0);
        assert_eq!(word_start("abc", 0), 0);
    }

    #[test]
    fn utf16_col_maps_to_byte_col() {
        // ASCII: byte == utf16.
        assert_eq!(utf16_to_byte_col("let x = 1;", 4), 4);
        assert_eq!(utf16_to_byte_col("abc", 0), 0);
        // "café" — 'é' is 1 UTF-16 unit but 2 bytes. Column after it:
        // utf16 4 → byte 5.
        assert_eq!(utf16_to_byte_col("café", 4), 5);
        assert_eq!(utf16_to_byte_col("café", 3), 3); // before 'é'
        // Emoji '😀' is 2 UTF-16 units (surrogate pair) and 4 bytes.
        assert_eq!(utf16_to_byte_col("a😀b", 3), 5); // a(1) + 😀(2) → byte 5
        // Past end clamps to len.
        assert_eq!(utf16_to_byte_col("hi", 99), 2);
    }
}
