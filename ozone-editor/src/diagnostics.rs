//! Diagnostics expressed as decorations.
//!
//! A diagnostic is a severity + message over a buffer range — the shape an LSP
//! server (Phase 3) or any linter produces. Rather than a parallel rendering
//! path, diagnostics are *published into the decoration store* under a caller
//! owned namespace: an underline over the range, a gutter sign on the start
//! line, and an end-of-line message. Re-publishing replaces the namespace's
//! prior set atomically, so a server can push fresh diagnostics each edit.
//!
//! This is the producer side the LSP client feeds; the renderer already draws
//! every decoration kind, so diagnostics display with no extra GUI code.

use ozone_buffer::{Buffer, BufferId, Pos};

use crate::decoration::{DecorationKind, DecorationStore, HlRole, NamespaceId, VirtualPos};

/// Diagnostic severity. Maps to a decoration [`HlRole`] (theme-coloured) and a
/// one-letter gutter sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
    Hint,
}

impl Severity {
    pub fn role(self) -> HlRole {
        match self {
            Severity::Error => HlRole::Error,
            Severity::Warn => HlRole::Warn,
            Severity::Info => HlRole::Info,
            Severity::Hint => HlRole::Hint,
        }
    }

    /// One-letter gutter sign.
    pub fn sign(self) -> &'static str {
        match self {
            Severity::Error => "E",
            Severity::Warn => "W",
            Severity::Info => "I",
            Severity::Hint => "H",
        }
    }
}

/// One diagnostic over a `[start, end)` position range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: Pos,
    pub end: Pos,
    pub severity: Severity,
    pub message: String,
    /// Optional producer label (e.g. `"rustc"`, `"rust-analyzer"`).
    pub source: Option<String>,
}

impl Diagnostic {
    pub fn new(start: Pos, end: Pos, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            start,
            end,
            severity,
            message: message.into(),
            source: None,
        }
    }
}

/// Replace every decoration in `namespace` for `buffer_id` with ones rendering
/// `diags`. Each diagnostic yields: an underline over its range (when non-empty),
/// a gutter sign on the start line, and an end-of-line virtual-text message.
pub fn publish(
    store: &mut DecorationStore,
    buf: &Buffer,
    buffer_id: BufferId,
    namespace: NamespaceId,
    diags: &[Diagnostic],
) {
    store.clear_namespace_in(buffer_id, namespace);
    for d in diags {
        let role = d.severity.role();
        let a = buf.pos_to_offset(d.start);
        let b = buf.pos_to_offset(d.end);
        let (start, end) = if a <= b { (a, b) } else { (b, a) };

        // Underline the span (only when it covers at least one byte).
        if end > start {
            store.add(
                buffer_id,
                namespace,
                start,
                end,
                DecorationKind::Underline(role),
            );
        }

        // Gutter sign on the start line.
        let line_start = buf.pos_to_offset(Pos::new(d.start.line, 0));
        store.add(
            buffer_id,
            namespace,
            line_start,
            line_start,
            DecorationKind::GutterSign(d.severity.sign().to_string()),
        );

        // End-of-line message.
        let eol = buf.pos_to_offset(Pos::new(d.start.line, buf.line_len(d.start.line)));
        store.add(
            buffer_id,
            namespace,
            eol,
            eol,
            DecorationKind::VirtualText {
                text: format!("  {}", d.message),
                pos: VirtualPos::Eol,
                role,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(line: usize, c0: usize, c1: usize, sev: Severity, msg: &str) -> Diagnostic {
        Diagnostic::new(Pos::new(line, c0), Pos::new(line, c1), sev, msg)
    }

    #[test]
    fn publish_creates_underline_sign_and_message() {
        let buf = Buffer::from_text("let x = 1;\nfn y() {}");
        let id = buf.id;
        let mut store = DecorationStore::new();
        let ns = store.namespace();
        publish(
            &mut store,
            &buf,
            id,
            ns,
            &[diag(0, 4, 5, Severity::Error, "unused")],
        );

        let kinds: Vec<&DecorationKind> = store.all(id).iter().map(|d| &d.kind).collect();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, DecorationKind::Underline(HlRole::Error)))
        );
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, DecorationKind::GutterSign(s) if s == "E"))
        );
        assert!(kinds.iter().any(|k| matches!(
            k,
            DecorationKind::VirtualText { pos: VirtualPos::Eol, role: HlRole::Error, text } if text.contains("unused")
        )));
    }

    #[test]
    fn republish_replaces_prior_set() {
        let buf = Buffer::from_text("aaaa\nbbbb");
        let id = buf.id;
        let mut store = DecorationStore::new();
        let ns = store.namespace();
        publish(
            &mut store,
            &buf,
            id,
            ns,
            &[diag(0, 0, 2, Severity::Warn, "first")],
        );
        let before = store.all(id).len();
        assert!(before > 0);
        publish(
            &mut store,
            &buf,
            id,
            ns,
            &[diag(1, 0, 1, Severity::Info, "second")],
        );
        // Old namespace decorations gone, only the new one's remain.
        assert!(store.all(id).iter().all(|d| match &d.kind {
            DecorationKind::VirtualText { text, .. } => text.contains("second"),
            _ => true,
        }));
    }

    #[test]
    fn empty_range_skips_underline_but_keeps_sign() {
        let buf = Buffer::from_text("xyz");
        let id = buf.id;
        let mut store = DecorationStore::new();
        let ns = store.namespace();
        publish(
            &mut store,
            &buf,
            id,
            ns,
            &[diag(0, 1, 1, Severity::Hint, "point")],
        );
        let kinds: Vec<&DecorationKind> = store.all(id).iter().map(|d| &d.kind).collect();
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, DecorationKind::Underline(_)))
        );
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, DecorationKind::GutterSign(_)))
        );
    }

    #[test]
    fn clearing_namespace_removes_diagnostics() {
        let buf = Buffer::from_text("hello");
        let id = buf.id;
        let mut store = DecorationStore::new();
        let ns = store.namespace();
        publish(
            &mut store,
            &buf,
            id,
            ns,
            &[diag(0, 0, 3, Severity::Error, "x")],
        );
        assert!(!store.all(id).is_empty());
        store.clear_namespace_in(id, ns);
        assert!(store.all(id).is_empty());
    }
}
