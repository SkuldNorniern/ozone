//! Full-document JSON(C) highlighter backed by the sylven lexer.
//!
//! [`scan_json_buffer`] lexes the whole source text once with
//! `sylven_lex::json::lex_json`, maps every token's byte range to a
//! `(line, column-within-line)` position, and returns one `Vec<TokenSpan>`
//! per line — the same format the renderer already consumes.

use sylven_lex::SyntaxKind;
use sylven_lex::json::{JsonKind, lex_json};

use crate::{TokenKind, TokenSpan};

/// Scan `text` with the sylven JSON lexer and return per-line [`TokenSpan`]s.
pub(super) fn scan_json_buffer(text: &str) -> Vec<Vec<TokenSpan>> {
    // Build a line-offset table so we can map byte positions to (line, col).
    let line_starts = build_line_starts(text);
    let line_count = line_starts.len();

    let mut result: Vec<Vec<TokenSpan>> = vec![Vec::new(); line_count];

    let stream = lex_json(text);
    for tok in stream.as_slice() {
        if tok.kind == SyntaxKind::EOF {
            break;
        }
        let kind = match json_kind_to_token(tok.kind) {
            Some(k) => k,
            None => continue,
        };

        let start = tok.range.start().to_usize();
        let end = tok.range.end().to_usize();

        // Find which line `start` falls on.
        let line_idx = line_starts
            .partition_point(|&ls| ls <= start)
            .saturating_sub(1);
        let line_start_byte = line_starts[line_idx];

        let line_len = line_starts
            .get(line_idx + 1)
            .map(|&s| s.saturating_sub(1) - line_start_byte)
            .unwrap_or(text.len() - line_start_byte);

        if end <= line_starts.get(line_idx + 1).copied().unwrap_or(text.len()) {
            // Whole token on one line.
            let col_start = start - line_start_byte;
            let col_end = end - line_start_byte;
            let len = col_end.min(line_len) - col_start;
            if len > 0 {
                result[line_idx].push(TokenSpan {
                    start: col_start,
                    len,
                    kind,
                });
            }
        } else {
            // Token spans multiple lines (multi-line block comment).
            let mut byte = start;
            let mut li = line_idx;
            while byte < end && li < line_count {
                let ls = line_starts[li];
                let le = line_starts
                    .get(li + 1)
                    .map(|&s| s.saturating_sub(1))
                    .unwrap_or(text.len());
                let col = byte - ls;
                let frag_end = end.min(le);
                let frag_len = frag_end - byte;
                if frag_len > 0 {
                    result[li].push(TokenSpan {
                        start: col,
                        len: frag_len,
                        kind,
                    });
                }
                byte = line_starts.get(li + 1).copied().unwrap_or(end);
                li += 1;
            }
        }
    }

    result
}

/// Convert a `SyntaxKind` from the sylven JSON lexer to an ozone `TokenKind`.
/// Returns `None` for whitespace (left `Default`).
fn json_kind_to_token(k: SyntaxKind) -> Option<TokenKind> {
    if k == SyntaxKind::WHITESPACE {
        return None;
    }
    if k == JsonKind::Key.to_syntax() {
        return Some(TokenKind::Keyword);
    }
    if k == JsonKind::String.to_syntax() {
        return Some(TokenKind::String);
    }
    if k == JsonKind::NumberLit.to_syntax() || k == JsonKind::BoolLit.to_syntax() {
        return Some(TokenKind::Number);
    }
    if k == JsonKind::NullLit.to_syntax() {
        return Some(TokenKind::Keyword);
    }
    if k == JsonKind::Comment.to_syntax() {
        return Some(TokenKind::Comment);
    }
    if k == JsonKind::Punctuation.to_syntax() {
        return Some(TokenKind::Punctuation);
    }
    if k == JsonKind::Ident.to_syntax() {
        return Some(TokenKind::Variable);
    }
    None // ERROR
}

/// Build a table of byte offsets where each line starts. `line_starts[i]` is
/// the byte offset of the first character on line `i` (0-based).
fn build_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_on_line(text: &str, line: usize) -> Vec<TokenKind> {
        scan_json_buffer(text)[line]
            .iter()
            .map(|s| s.kind)
            .collect()
    }

    #[test]
    fn key_vs_string_value() {
        let ks = kinds_on_line(r#"{"name": "ozone"}"#, 0);
        assert_eq!(
            ks,
            vec![
                TokenKind::Punctuation,
                TokenKind::Keyword,
                TokenKind::Punctuation,
                TokenKind::String,
                TokenKind::Punctuation,
            ]
        );
    }

    #[test]
    fn numbers_and_literals() {
        let ks = kinds_on_line(r#"{"n": 42, "ok": true, "x": null}"#, 0);
        assert!(ks.contains(&TokenKind::Number));
        // keys "n"/"ok"/"x" + the `null` literal all map to Keyword.
        assert_eq!(ks.iter().filter(|&&k| k == TokenKind::Keyword).count(), 4);
    }

    #[test]
    fn line_comment() {
        let ks = kinds_on_line("// hello", 0);
        assert_eq!(ks, vec![TokenKind::Comment]);
    }

    #[test]
    fn multiline_block_comment_spans_lines() {
        let result = scan_json_buffer("/* a\nb */1");
        assert_eq!(
            result[0],
            vec![TokenSpan {
                start: 0,
                len: 4,
                kind: TokenKind::Comment
            }]
        );
        assert!(
            result[1]
                .iter()
                .any(|s| s.kind == TokenKind::Comment && s.start == 0)
        );
        assert!(result[1].iter().any(|s| s.kind == TokenKind::Number));
    }

    #[test]
    fn line_count_matches_newlines() {
        let text = "{\n  \"a\": 1\n}";
        let result = scan_json_buffer(text);
        assert_eq!(result.len(), 3);
    }
}
