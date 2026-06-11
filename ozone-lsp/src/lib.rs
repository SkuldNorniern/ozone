//! Ozone's Language Server Protocol client.
//!
//! Built bottom-up and dependency-free (no serde, no async runtime), matching
//! the project's hand-parsed-config philosophy:
//!
//! - [`json`] — a minimal JSON value + parser + serializer (the wire format).
//! - [`rpc`] — JSON-RPC framing (`Content-Length` headers) + stream splitting.
//! - [`protocol`] — decode server messages into editor types (diagnostics).
//! - [`client`] — the live connection: server process + handshake + reader
//!   thread, exposing `didOpen`/`didChange` and a [`ServerMessage`] channel.
//!
//! The client is a *producer*: it feeds diagnostics/hints into the editor's
//! decoration store and answers requests; it never draws.

pub mod client;
pub mod json;
pub mod protocol;
pub mod rpc;

pub use client::{LspClient, ServerMessage};
pub use json::{Json, parse as parse_json};
pub use protocol::{Location, parse_goto_definition_result, parse_hover_result, parse_publish_diagnostics};
