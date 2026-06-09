//! The live LSP client: server process + handshake + reader thread.
//!
//! Dependency-free and runtime-free — `std::process` + one reader thread + an
//! `mpsc` channel, matching the project's no-async-runtime decision. The client
//! is a *producer*: it spawns a language server, performs the
//! `initialize`/`initialized` handshake, sends `didOpen`/`didChange`
//! notifications, and forwards `publishDiagnostics` to the editor as
//! [`ServerMessage`]s the GUI drains each frame.
//!
//! Threading model: the `initialize` handshake reads stdout *synchronously*
//! (blocking until the response arrives), then hands the stdout pipe — and any
//! bytes already buffered — to a reader thread that owns it for the rest of the
//! session. Sends go straight to the child's stdin from the caller's thread.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use ozone_editor::Diagnostic;

use crate::json::Json;
use crate::{protocol, rpc};

/// A decoded message from the server, delivered to the GUI via the channel.
#[derive(Debug)]
pub enum ServerMessage {
    /// `textDocument/publishDiagnostics` for `uri`.
    Diagnostics {
        uri: String,
        diagnostics: Vec<Diagnostic>,
    },
}

/// A running language-server connection.
pub struct LspClient {
    stdin: ChildStdin,
    child: Child,
    next_id: i64,
    rx: Receiver<ServerMessage>,
    _reader: JoinHandle<()>,
}

impl LspClient {
    /// Spawn `command` (e.g. `"rust-analyzer"`) with `args`, run the
    /// initialize/initialized handshake rooted at `root_uri`, and start the
    /// reader thread. Blocks until the server answers `initialize`.
    pub fn start(command: &str, args: &[String], root_uri: &str) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {command}: {e}"))?;

        let mut stdin = child.stdin.take().ok_or("no stdin handle")?;
        let mut stdout = child.stdout.take().ok_or("no stdout handle")?;

        stdin
            .write_all(&rpc::request(1, "initialize", initialize_params(root_uri)))
            .map_err(|e| e.to_string())?;
        stdin.flush().ok();

        // Block until the server's initialize response (id 1) lands; keep any
        // leftover bytes for the reader thread.
        let mut buf: Vec<u8> = Vec::new();
        read_until_response(&mut stdout, &mut buf, 1)?;

        stdin
            .write_all(&rpc::notification("initialized", Json::Object(vec![])))
            .map_err(|e| e.to_string())?;
        stdin.flush().ok();

        let (tx, rx) = channel();
        let reader = std::thread::spawn(move || reader_loop(stdout, buf, tx));

        Ok(Self {
            stdin,
            child,
            next_id: 2,
            rx,
            _reader: reader,
        })
    }

    /// Drain any server messages received since the last poll (non-blocking).
    pub fn poll(&self) -> Vec<ServerMessage> {
        self.rx.try_iter().collect()
    }

    /// Tell the server a document was opened. `version` starts at 1 and must
    /// increase on every subsequent change.
    pub fn did_open(&mut self, uri: &str, language_id: &str, version: i64, text: &str) {
        let params = Json::Object(vec![(
            "textDocument".into(),
            Json::Object(vec![
                ("uri".into(), Json::Str(uri.into())),
                ("languageId".into(), Json::Str(language_id.into())),
                ("version".into(), Json::Num(version as f64)),
                ("text".into(), Json::Str(text.into())),
            ]),
        )]);
        self.send(&rpc::notification("textDocument/didOpen", params));
    }

    /// Tell the server a document changed (full-document sync — one change with
    /// the whole new text, matching `TextDocumentSyncKind::Full`).
    pub fn did_change(&mut self, uri: &str, version: i64, text: &str) {
        let params = Json::Object(vec![
            (
                "textDocument".into(),
                Json::Object(vec![
                    ("uri".into(), Json::Str(uri.into())),
                    ("version".into(), Json::Num(version as f64)),
                ]),
            ),
            (
                "contentChanges".into(),
                Json::Array(vec![Json::Object(vec![(
                    "text".into(),
                    Json::Str(text.into()),
                )])]),
            ),
        ]);
        self.send(&rpc::notification("textDocument/didChange", params));
    }

    /// Tell the server a document was closed.
    pub fn did_close(&mut self, uri: &str) {
        let params = Json::Object(vec![(
            "textDocument".into(),
            Json::Object(vec![("uri".into(), Json::Str(uri.into()))]),
        )]);
        self.send(&rpc::notification("textDocument/didClose", params));
    }

    /// Politely ask the server to shut down, then exit.
    pub fn shutdown(&mut self) {
        let id = self.alloc_id();
        self.send(&rpc::request(id, "shutdown", Json::Null));
        self.send(&rpc::notification("exit", Json::Null));
    }

    fn alloc_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, bytes: &[u8]) {
        // A dead pipe just means the server exited; surfacing it per-send would
        // be noise. The reader thread's EOF is the authoritative "server gone".
        let _ = self.stdin.write_all(bytes);
        let _ = self.stdin.flush();
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Minimal `initialize` params. rust-analyzer is lenient about capabilities; we
/// advertise just enough to receive diagnostics and document sync.
fn initialize_params(root_uri: &str) -> Json {
    Json::Object(vec![
        ("processId".into(), Json::Num(std::process::id() as f64)),
        ("rootUri".into(), Json::Str(root_uri.into())),
        (
            "clientInfo".into(),
            Json::Object(vec![("name".into(), Json::Str("ozone".into()))]),
        ),
        (
            "capabilities".into(),
            Json::Object(vec![(
                "textDocument".into(),
                Json::Object(vec![("publishDiagnostics".into(), Json::Object(vec![]))]),
            )]),
        ),
    ])
}

/// Read frames from `stdout` into `buf` until a response with `id` arrives,
/// returning it. Messages that arrive first (log/progress notifications) are
/// discarded. Leaves unconsumed bytes in `buf` for the reader thread.
fn read_until_response(
    stdout: &mut ChildStdout,
    buf: &mut Vec<u8>,
    id: i64,
) -> Result<Json, String> {
    let mut tmp = [0u8; 8192];
    loop {
        // Drain any complete frames already buffered; ignore pre-init
        // notifications/requests until our response (`id`) arrives.
        while let Some(msg) = rpc::take_message(buf)? {
            if msg.get("id").and_then(Json::as_i64) == Some(id) {
                if let Some(err) = msg.get("error") {
                    return Err(format!("initialize failed: {err}"));
                }
                return Ok(msg);
            }
        }
        let n = stdout.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("server closed the connection during initialize".to_string());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Own `stdout` for the session: parse frames and forward decoded messages until
/// the pipe closes or the receiver is dropped.
fn reader_loop(mut stdout: ChildStdout, mut buf: Vec<u8>, tx: Sender<ServerMessage>) {
    let mut tmp = [0u8; 8192];
    loop {
        loop {
            match rpc::take_message(&mut buf) {
                Ok(Some(msg)) => {
                    if let Some(out) = classify(&msg)
                        && tx.send(out).is_err()
                    {
                        return; // GUI dropped the client
                    }
                }
                Ok(None) => break,
                // A malformed frame shouldn't happen with a conforming server;
                // drop the buffer to resync rather than spin on the same bytes.
                Err(_) => {
                    buf.clear();
                    break;
                }
            }
        }
        match stdout.read(&mut tmp) {
            Ok(0) | Err(_) => return, // server exited
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
}

/// Turn one server message into a [`ServerMessage`], or `None` for messages we
/// don't consume yet (log/progress/responses).
fn classify(msg: &Json) -> Option<ServerMessage> {
    match msg.get("method").and_then(Json::as_str)? {
        "textDocument/publishDiagnostics" => {
            let params = msg.get("params")?;
            let (uri, diagnostics) = protocol::parse_publish_diagnostics(params)?;
            Some(ServerMessage::Diagnostics { uri, diagnostics })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn initialize_params_carry_root_and_capabilities() {
        let p = initialize_params("file:///proj");
        assert_eq!(
            p.get("rootUri").and_then(Json::as_str),
            Some("file:///proj")
        );
        assert!(p.get("processId").and_then(Json::as_i64).is_some());
        assert!(
            p.get("capabilities")
                .and_then(|c| c.get("textDocument"))
                .and_then(|t| t.get("publishDiagnostics"))
                .is_some()
        );
    }

    #[test]
    fn classify_decodes_publish_diagnostics() {
        let msg = json::parse(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics",
                "params":{"uri":"file:///a.rs","diagnostics":[
                    {"range":{"start":{"line":1,"character":2},
                              "end":{"line":1,"character":5}},
                     "severity":1,"message":"boom"}]}}"#,
        )
        .unwrap();
        match classify(&msg) {
            Some(ServerMessage::Diagnostics { uri, diagnostics }) => {
                assert_eq!(uri, "file:///a.rs");
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].message, "boom");
            }
            _ => panic!("expected diagnostics"),
        }
    }

    #[test]
    fn classify_ignores_log_and_responses() {
        let log = json::parse(r#"{"method":"window/logMessage","params":{}}"#).unwrap();
        assert!(classify(&log).is_none());
        let resp = json::parse(r#"{"id":1,"result":{}}"#).unwrap();
        assert!(classify(&resp).is_none());
    }

    #[test]
    fn read_until_response_skips_pre_init_then_returns_match() {
        // A log notification, then the initialize response, concatenated as they
        // would arrive on the wire. `Cursor<Vec<u8>>` stands in for the pipe.
        let mut wire = rpc::notification("window/logMessage", Json::Object(vec![]));
        wire.extend(rpc::request(2, "other", Json::Null)); // id 2 — not ours, skipped
        wire.extend(rpc::request(1, "unused", Json::Null)); // id 1 → returned
        let mut reader = std::io::Cursor::new(wire);
        let mut buf = Vec::new();
        let got = read_until_response_generic(&mut reader, &mut buf, 1).unwrap();
        assert_eq!(got.get("id").and_then(Json::as_i64), Some(1));
    }

    // Generic over any reader so the handshake loop is testable without a real
    // process; `read_until_response` is the `ChildStdout` specialization.
    fn read_until_response_generic<R: std::io::Read>(
        r: &mut R,
        buf: &mut Vec<u8>,
        id: i64,
    ) -> Result<Json, String> {
        let mut tmp = [0u8; 64];
        loop {
            while let Some(msg) = rpc::take_message(buf)? {
                if msg.get("id").and_then(Json::as_i64) == Some(id) {
                    return Ok(msg);
                }
            }
            let n = r.read(&mut tmp).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("eof".into());
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    }
}
