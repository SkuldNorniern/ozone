//! Ozone's Language Server Protocol client.
//!
//! Built bottom-up and dependency-free (no serde, no async runtime), matching
//! the project's hand-parsed-config philosophy:
//!
//! - [`json`] — a minimal JSON value + parser + serializer (the wire format).
//! - JSON-RPC framing (`Content-Length` headers), the server process lifecycle,
//!   the capability handshake, and turning `publishDiagnostics` notifications
//!   into [`ozone_editor::Diagnostic`]s are layered on top (follow-up slices).
//!
//! The client is a *producer*: it feeds diagnostics/hints into the editor's
//! decoration store and answers requests; it never draws.

pub mod json;
pub mod protocol;
pub mod rpc;

pub use json::{Json, parse as parse_json};
pub use protocol::parse_publish_diagnostics;
