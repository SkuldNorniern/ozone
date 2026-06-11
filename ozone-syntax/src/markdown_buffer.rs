//! Full-document Markdown highlighter backed by the sylven lexer.
//!
//! [`scan_markdown_buffer`] lexes the whole source text once with
//! `sylven_lex::markdown::lex_markdown`, maps every token's byte range to a
//! `(line, column-within-line)` position, and returns one `Vec<TokenSpan>`
//! per line — the same format the renderer already consumes.

use sylven_lex::SyntaxKind;
use sylven_lex::markdown::{MarkdownKind, lex_markdown};

use crate::{TokenKind, TokenSpan};

/// Scan `text` with the sylven Markdown lexer and return per-line [`TokenSpan`]s.
pub(super) fn scan_markdown_buffer(text: &str) -> Vec<Vec<TokenSpan>> {
    // Build a line-offset table so we can map byte positions to (line, col).
    let line_starts = build_line_starts(text);
    let line_count = line_starts.len();

    let mut result: Vec<Vec<TokenSpan>> = vec![Vec::new(); line_count];

    let stream = lex_markdown(text);
    for tok in stream.as_slice() {
        if tok.kind == SyntaxKind::EOF {
            break;
        }
        // Skip trivia (whitespace) and plain text — gaps are implicitly
        // TokenKind::Default.
        let kind = match markdown_kind_to_token(tok.kind) {
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
            // Token spans multiple lines (multi-line code span is not
            // possible, but kept for consistency with the other buffers).
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

/// Convert a `SyntaxKind` from the sylven Markdown lexer to an ozone
/// `TokenKind`. Returns `None` for whitespace, plain text, and code-block
/// body lines (left `Default`).
fn markdown_kind_to_token(k: SyntaxKind) -> Option<TokenKind> {
    if k == SyntaxKind::WHITESPACE {
        return None;
    }
    if k == MarkdownKind::Heading.to_syntax() {
        return Some(TokenKind::Keyword);
    }
    if k == MarkdownKind::BlockQuote.to_syntax() || k == MarkdownKind::CodeFenceDelim.to_syntax() {
        return Some(TokenKind::Comment);
    }
    if k == MarkdownKind::ListMarker.to_syntax() {
        return Some(TokenKind::Operator);
    }
    if k == MarkdownKind::CodeSpan.to_syntax() || k == MarkdownKind::LinkUrl.to_syntax() {
        return Some(TokenKind::String);
    }
    if k == MarkdownKind::LinkText.to_syntax() {
        return Some(TokenKind::Function);
    }
    // Text, CodeBlockBody, ERROR → Default
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
        scan_markdown_buffer(text)[line]
            .iter()
            .map(|s| s.kind)
            .collect()
    }

    #[test]
    fn heading_classified() {
        let ks = kinds_on_line("## Title", 0);
        assert_eq!(ks, vec![TokenKind::Keyword]);
    }

    #[test]
    fn blockquote_classified() {
        let ks = kinds_on_line("> quoted", 0);
        assert_eq!(ks, vec![TokenKind::Comment]);
    }

    #[test]
    fn list_marker_and_text() {
        let ks = kinds_on_line("- item", 0);
        assert_eq!(ks, vec![TokenKind::Operator]);
    }

    #[test]
    fn code_span_and_link() {
        let ks = kinds_on_line("see `code` and [text](http://x)", 0);
        assert!(ks.contains(&TokenKind::String)); // `code` and (url)
        assert!(ks.contains(&TokenKind::Function)); // [text]
    }

    #[test]
    fn fenced_code_block_body_is_default() {
        let result = scan_markdown_buffer("```rust\nlet x = 1;\n```\n");
        assert_eq!(
            result[0],
            vec![TokenSpan {
                start: 0,
                len: 7,
                kind: TokenKind::Comment
            }]
        );
        assert!(result[1].is_empty());
        assert_eq!(
            result[2],
            vec![TokenSpan {
                start: 0,
                len: 3,
                kind: TokenKind::Comment
            }]
        );
    }

    #[test]
    fn line_count_matches_newlines() {
        let text = "a\nb\nc";
        let result = scan_markdown_buffer(text);
        assert_eq!(result.len(), 3);
    }
}
