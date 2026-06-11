//! Full-document Rust highlighter backed by the sylven lexer.
//!
//! [`scan_rust_buffer`] lexes the whole source text once with
//! `sylven_lex::rust::lex_rust`, maps every token's byte range to a
//! `(line, column-within-line)` position, and returns one `Vec<TokenSpan>`
//! per line — the same format the renderer already consumes.

use sylven_lex::SyntaxKind;
use sylven_lex::rust::{RustKind, lex_rust};

use crate::{TokenKind, TokenSpan};

/// Scan `text` with the sylven Rust lexer and return per-line [`TokenSpan`]s.
pub(super) fn scan_rust_buffer(text: &str) -> Vec<Vec<TokenSpan>> {
    // Build a line-offset table so we can map byte positions to (line, col).
    let line_starts = build_line_starts(text);
    let line_count = line_starts.len();

    let mut result: Vec<Vec<TokenSpan>> = vec![Vec::new(); line_count];

    let stream = lex_rust(text);
    for tok in stream.as_slice() {
        if tok.kind == SyntaxKind::EOF {
            break;
        }
        // Skip trivia (whitespace) — gaps are implicitly TokenKind::Default.
        let kind = match rust_kind_to_token(tok.kind) {
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

        let tok_start_col = start - line_start_byte;
        let tok_end_col = end.min(
            line_starts
                .get(line_idx + 1)
                .map(|&s| s.saturating_sub(1)) // exclude `\n`
                .unwrap_or(text.len()),
        );

        if tok_end_col <= line_start_byte + tok_start_col {
            // Degenerate / zero-length span — skip.
            continue;
        }

        // If the token doesn't cross a line boundary, emit one span.
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
            // Token spans multiple lines (block comment, multi-line string).
            // Emit a fragment per line.
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

/// Convert a `SyntaxKind` from the sylven Rust lexer to an ozone `TokenKind`.
/// Returns `None` for whitespace (trivia) and line/block comment trivia.
fn rust_kind_to_token(k: SyntaxKind) -> Option<TokenKind> {
    if k == SyntaxKind::WHITESPACE {
        return None;
    }
    if k == RustKind::Keyword.to_syntax() {
        return Some(TokenKind::Keyword);
    }
    if k == RustKind::KeywordControl.to_syntax() {
        return Some(TokenKind::KeywordControl);
    }
    if k == RustKind::PrimitiveType.to_syntax() || k == RustKind::StdType.to_syntax() {
        return Some(TokenKind::Type);
    }
    if k == RustKind::BoolLit.to_syntax() {
        return Some(TokenKind::Number); // same as existing scanner
    }
    if k == RustKind::Lifetime.to_syntax() {
        return Some(TokenKind::Lifetime);
    }
    if k == RustKind::StringLit.to_syntax() || k == RustKind::CharLit.to_syntax() {
        return Some(TokenKind::String);
    }
    if k == RustKind::NumberLit.to_syntax() {
        return Some(TokenKind::Number);
    }
    if k == RustKind::LineComment.to_syntax() || k == RustKind::BlockComment.to_syntax() {
        return Some(TokenKind::Comment);
    }
    if k == RustKind::MacroIdent.to_syntax() {
        return Some(TokenKind::Macro);
    }
    if k == RustKind::Attribute.to_syntax() {
        return Some(TokenKind::Attribute);
    }
    if k == RustKind::FunctionIdent.to_syntax() {
        return Some(TokenKind::Function);
    }
    if k == RustKind::PascalIdent.to_syntax() {
        return Some(TokenKind::Type);
    }
    if k == RustKind::Operator.to_syntax() {
        return Some(TokenKind::Operator);
    }
    if k == RustKind::Punctuation.to_syntax() {
        return Some(TokenKind::Punctuation);
    }
    if k == RustKind::Ident.to_syntax() {
        return Some(TokenKind::Variable);
    }
    // SyntaxKind::ERROR, SyntaxKind::COMMENT (trivia comment) → skip
    None
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
        scan_rust_buffer(text)[line]
            .iter()
            .map(|s| s.kind)
            .collect()
    }

    #[test]
    fn basic_keyword_on_first_line() {
        let result = scan_rust_buffer("fn main() {}");
        assert!(result[0].iter().any(|s| s.kind == TokenKind::Keyword));
        assert!(result[0].iter().any(|s| s.kind == TokenKind::Function));
    }

    #[test]
    fn multiline_block_comment_spans_both_lines() {
        let text = "/* start\nend */done";
        let result = scan_rust_buffer(text);
        assert!(result[0].iter().any(|s| s.kind == TokenKind::Comment));
        assert!(result[1].iter().any(|s| s.kind == TokenKind::Comment));
    }

    #[test]
    fn string_literal_classified_correctly() {
        let ks = kinds_on_line(r#"let s = "hello";"#, 0);
        assert!(ks.contains(&TokenKind::Keyword)); // let
        assert!(ks.contains(&TokenKind::String)); // "hello"
    }

    #[test]
    fn macro_classified() {
        let ks = kinds_on_line("println!(\"hi\");", 0);
        assert!(ks.contains(&TokenKind::Macro));
    }

    #[test]
    fn attribute_classified() {
        let ks = kinds_on_line("#[derive(Debug)]", 0);
        assert!(ks.contains(&TokenKind::Attribute));
    }

    #[test]
    fn line_count_matches_newlines() {
        let text = "a\nb\nc";
        let result = scan_rust_buffer(text);
        assert_eq!(result.len(), 3);
    }
}
