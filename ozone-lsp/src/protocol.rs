//! Decode the LSP messages Ozone consumes into editor types.
//!
//! Currently `textDocument/publishDiagnostics` → `(uri, Vec<Diagnostic>)`, ready
//! to hand to `Workspace::publish_diagnostics`. More request/response decoders
//! (hover, completion, definition) layer in here as they are wired.
//!
//! NOTE: LSP `character` is a UTF-16 code-unit offset; Ozone columns are byte
//! offsets. This maps them directly, which is correct for ASCII. UTF-16↔byte
//! reconciliation (using the line text) is a tracked follow-up.

use ozone_buffer::Pos;
use ozone_editor::{Diagnostic, Severity};

use crate::json::Json;

fn severity_from(code: i64) -> Severity {
    match code {
        1 => Severity::Error,
        2 => Severity::Warn,
        3 => Severity::Info,
        4 => Severity::Hint,
        _ => Severity::Info,
    }
}

/// Read an LSP `Position` (`{line, character}`, both 0-based).
fn position(value: &Json) -> Pos {
    let line = value.get("line").and_then(Json::as_i64).unwrap_or(0).max(0) as usize;
    let character = value
        .get("character")
        .and_then(Json::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    Pos::new(line, character)
}

/// One LSP `Diagnostic` object → an editor [`Diagnostic`].
fn diagnostic(value: &Json) -> Option<Diagnostic> {
    let range = value.get("range")?;
    let start = position(range.get("start")?);
    let end = position(range.get("end")?);
    let severity = value
        .get("severity")
        .and_then(Json::as_i64)
        .map(severity_from)
        .unwrap_or(Severity::Info);
    let message = value
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    let source = value
        .get("source")
        .and_then(Json::as_str)
        .map(str::to_string);
    Some(Diagnostic {
        start,
        end,
        severity,
        message,
        source,
    })
}

/// Decode `textDocument/publishDiagnostics` params into `(uri, diagnostics)`.
/// Returns `None` if the `uri` is missing; an absent/empty `diagnostics` array
/// yields an empty `Vec` (which clears the buffer's set when republished).
pub fn parse_publish_diagnostics(params: &Json) -> Option<(String, Vec<Diagnostic>)> {
    let uri = params.get("uri").and_then(Json::as_str)?.to_string();
    let diagnostics = params
        .get("diagnostics")
        .and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(diagnostic)
        .collect();
    Some((uri, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn parses_publish_diagnostics() {
        let params = json::parse(
            r#"{
                "uri": "file:///x/main.rs",
                "diagnostics": [
                    {
                        "range": {"start": {"line": 2, "character": 4},
                                  "end":   {"line": 2, "character": 9}},
                        "severity": 1,
                        "source": "rustc",
                        "message": "cannot find value `foo`"
                    },
                    {
                        "range": {"start": {"line": 5, "character": 0},
                                  "end":   {"line": 5, "character": 0}},
                        "severity": 2,
                        "message": "unused import"
                    }
                ]
            }"#,
        )
        .unwrap();

        let (uri, diags) = parse_publish_diagnostics(&params).unwrap();
        assert_eq!(uri, "file:///x/main.rs");
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].start, Pos::new(2, 4));
        assert_eq!(diags[0].end, Pos::new(2, 9));
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].source.as_deref(), Some("rustc"));
        assert!(diags[0].message.contains("foo"));
        assert_eq!(diags[1].severity, Severity::Warn);
        assert_eq!(diags[1].source, None);
    }

    #[test]
    fn missing_uri_is_none_empty_diags_ok() {
        let p = json::parse(r#"{"diagnostics":[]}"#).unwrap();
        assert!(parse_publish_diagnostics(&p).is_none());
        let p = json::parse(r#"{"uri":"file:///a","diagnostics":[]}"#).unwrap();
        let (_, diags) = parse_publish_diagnostics(&p).unwrap();
        assert!(diags.is_empty());
    }

    #[test]
    fn unknown_severity_defaults_to_info() {
        let p = json::parse(
            r#"{"uri":"file:///a","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"x"}]}"#,
        )
        .unwrap();
        let (_, diags) = parse_publish_diagnostics(&p).unwrap();
        assert_eq!(diags[0].severity, Severity::Info);
    }
}
