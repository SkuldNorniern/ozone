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

/// An LSP `Location` — a URI plus the start of a range. Used by
/// `textDocument/definition` and similar request features.
/// `character` is a UTF-16 code-unit offset as received from the server;
/// callers remap to byte columns via [`crate::lsp::utf16_to_byte_col`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    /// 0-based line.
    pub line: usize,
    /// 0-based character (UTF-16 code units).
    pub character: usize,
}

fn location(value: &Json) -> Option<Location> {
    // Accept `Location` (has `uri`) or `LocationLink` (has `targetUri`).
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))
        .and_then(Json::as_str)?
        .to_string();
    // LocationLink: prefer `targetSelectionRange`; Location: `range`.
    let range = value
        .get("targetSelectionRange")
        .or_else(|| value.get("range"))?;
    let start = range.get("start")?;
    let line = start.get("line").and_then(Json::as_i64).unwrap_or(0).max(0) as usize;
    let character = start
        .get("character")
        .and_then(Json::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    Some(Location {
        uri,
        line,
        character,
    })
}

/// Decode a `textDocument/definition` result.
///
/// The LSP spec allows `Location | Location[] | LocationLink[] | null`.
pub fn parse_goto_definition_result(result: &Json) -> Vec<Location> {
    match result {
        Json::Null => vec![],
        Json::Object(_) => location(result).into_iter().collect(),
        Json::Array(items) => items.iter().filter_map(location).collect(),
        _ => vec![],
    }
}

/// Decode a `textDocument/hover` result into plain text.
///
/// The spec allows:
/// - `null`
/// - `{ kind, value }` (`MarkupContent`)
/// - legacy `string` or `{ language, value }` (`MarkedString`)
/// - array of the above
///
/// All forms are flattened to a single string for display.
pub fn parse_hover_result(result: &Json) -> Option<String> {
    fn extract(v: &Json) -> Option<String> {
        match v {
            Json::Str(s) => Some(s.clone()),
            Json::Object(_) => {
                // MarkupContent { kind, value } or MarkedString { language, value }
                v.get("value").and_then(Json::as_str).map(str::to_string)
            }
            _ => None,
        }
    }

    match result {
        Json::Null => None,
        Json::Array(items) => {
            let joined: Vec<String> = items.iter().filter_map(extract).collect();
            if joined.is_empty() {
                None
            } else {
                Some(joined.join("\n\n"))
            }
        }
        other => extract(other),
    }
}

/// Subset of `ServerCapabilities` Ozone acts on, decoded once from the
/// `initialize` response so request features can check support before sending
/// a request the server would reject or ignore.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerCapabilities {
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub rename: bool,
    pub code_action: bool,
    pub document_formatting: bool,
    pub completion: bool,
    pub inlay_hint: bool,
}

/// A `*Provider` field is "supported" if present and not `false`/`null` —
/// servers may advertise either `true` or an options object.
fn provider_enabled(capabilities: &Json, key: &str) -> bool {
    match capabilities.get(key) {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(_) => true,
    }
}

/// Decode the `capabilities` object from an `initialize` response's `result`.
pub fn parse_server_capabilities(capabilities: &Json) -> ServerCapabilities {
    ServerCapabilities {
        hover: provider_enabled(capabilities, "hoverProvider"),
        definition: provider_enabled(capabilities, "definitionProvider"),
        references: provider_enabled(capabilities, "referencesProvider"),
        rename: provider_enabled(capabilities, "renameProvider"),
        code_action: provider_enabled(capabilities, "codeActionProvider"),
        document_formatting: provider_enabled(capabilities, "documentFormattingProvider"),
        completion: provider_enabled(capabilities, "completionProvider"),
        inlay_hint: provider_enabled(capabilities, "inlayHintProvider"),
    }
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
    fn parse_goto_definition_location() {
        // Single Location object.
        let single = json::parse(
            r#"{"uri":"file:///src/lib.rs","range":{"start":{"line":10,"character":4},"end":{"line":10,"character":8}}}"#,
        ).unwrap();
        let locs = parse_goto_definition_result(&single);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, "file:///src/lib.rs");
        assert_eq!(locs[0].line, 10);
        assert_eq!(locs[0].character, 4);
    }

    #[test]
    fn parse_goto_definition_array_and_null() {
        // Array of two Location objects.
        let arr = json::parse(
            r#"[{"uri":"file:///a.rs","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},
               {"uri":"file:///b.rs","range":{"start":{"line":5,"character":2},"end":{"line":5,"character":3}}}]"#,
        ).unwrap();
        let locs = parse_goto_definition_result(&arr);
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[1].uri, "file:///b.rs");
        assert_eq!(locs[1].line, 5);

        // Null result (definition not found).
        assert!(parse_goto_definition_result(&Json::Null).is_empty());
    }

    #[test]
    fn parse_hover_markup_content() {
        let markup = json::parse(
            r#"{"contents":{"kind":"markdown","value":"**fn foo**\n\nDoes the thing."}}"#,
        )
        .unwrap();
        let text = parse_hover_result(markup.get("contents").unwrap()).unwrap();
        assert!(text.contains("fn foo"));
        assert!(text.contains("Does the thing."));
    }

    #[test]
    fn parse_hover_legacy_string_and_null() {
        let s = Json::Str("hello".to_string());
        assert_eq!(parse_hover_result(&s).as_deref(), Some("hello"));
        assert!(parse_hover_result(&Json::Null).is_none());
    }

    #[test]
    fn server_capabilities_decode_bool_and_object_providers() {
        let caps = json::parse(
            r#"{
                "hoverProvider": true,
                "definitionProvider": {"workDoneProgress": false},
                "referencesProvider": false,
                "completionProvider": {"triggerCharacters": ["."]}
            }"#,
        )
        .unwrap();
        let parsed = parse_server_capabilities(&caps);
        assert!(parsed.hover);
        assert!(parsed.definition);
        assert!(!parsed.references);
        assert!(parsed.completion);
        assert!(!parsed.rename);
        assert!(!parsed.code_action);
        assert!(!parsed.document_formatting);
        assert!(!parsed.inlay_hint);
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
