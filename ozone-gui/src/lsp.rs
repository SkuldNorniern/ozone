//! GUI-side LSP orchestration: lazily start the configured server, mirror open
//! Rust buffers to it (`didOpen`/`didChange`/`didClose`), and route incoming
//! diagnostics into the decoration store via `Workspace::publish_diagnostics`.
//!
//! The editor core stays transport-free; this module is the single place the
//! frontend drives the `ozone_lsp` client. `sync` is called once per frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ozone_buffer::{Buffer, BufferId, BufferKind, Pos};
use ozone_config::Config;
use ozone_editor::{Diagnostic, NamespaceId, NotifyLevel, Workspace};
use ozone_lsp::{LspClient, ServerMessage};
use ozone_syntax::Filetype;

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

/// Frontend LSP state. Holds at most one server for now (Rust); generalizing to
/// a server-per-language map is a later step.
#[derive(Default)]
pub(crate) struct Lsp {
    client: Option<LspClient>,
    /// True once a start attempt failed, so we don't respawn on every frame.
    failed: bool,
    /// Diagnostics decoration namespace, allocated on first start.
    namespace: Option<NamespaceId>,
    docs: HashMap<BufferId, Doc>,
}

impl Lsp {
    pub(crate) fn new() -> Self {
        Self::default()
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

        // Lazy start: only once a Rust file is actually open.
        if self.client.is_none() {
            if open.is_empty() {
                return false;
            }
            let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match LspClient::start(&cfg.server, &cfg.args, &path_to_uri(&root)) {
                Ok(client) => {
                    self.client = Some(client);
                    self.namespace = Some(ws.decorations_mut().namespace());
                }
                Err(e) => {
                    self.failed = true;
                    ws.notify(NotifyLevel::Warn, format!("LSP: {e}"));
                    return true;
                }
            }
        }

        let mut changed = false;
        changed |= self.open_and_update(ws, &open);
        changed |= self.close_gone(ws, &open);
        changed |= self.drain_diagnostics(ws);
        changed
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

    /// Drain server messages and publish diagnostics, matching each `uri` back to
    /// an open buffer by canonical path.
    fn drain_diagnostics(&mut self, ws: &mut Workspace) -> bool {
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
            let ServerMessage::Diagnostics { uri, diagnostics } = msg;
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
            // LSP columns are UTF-16 code units; remap to byte columns first.
            let remapped = remap_to_byte_cols(ws, id, diagnostics);
            ws.publish_diagnostics(id, ns, &remapped);
            changed = true;
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

/// Open file buffers whose filetype is Rust.
fn rust_file_buffers(ws: &Workspace) -> Vec<(BufferId, PathBuf)> {
    ws.buffers
        .iter()
        .filter_map(|(id, b)| match &b.kind {
            BufferKind::File(p) if Filetype::from_path(&p.to_string_lossy()) == Filetype::Rust => {
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
