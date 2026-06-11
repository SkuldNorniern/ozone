//! Full-document highlighter for sylven DSL-compiled plugins (C, Python,
//! Oxygen, …) that have no dedicated Layer-0 buffer scanner.
//!
//! [`scan_dsl_buffer`] runs the language's sylven plugin once via
//! [`crate::parse_features`] and converts its whole-buffer
//! [`sylven::Highlight`] ranges into per-line [`TokenSpan`]s.

use sylven::HighlightKind;
use taste::Language;

use crate::{TokenKind, TokenSpan, parse_features};

/// Highlight `text` for `lang` using its sylven plugin, if one is registered.
/// Returns `None` when `lang` has no plugin (caller should fall back).
pub(super) fn scan_dsl_buffer(lang: Language, text: &str) -> Option<Vec<Vec<TokenSpan>>> {
    let features = parse_features(Some(lang), text)?;
    let line_starts = build_line_starts(text);
    let line_count = line_starts.len();
    let mut result: Vec<Vec<TokenSpan>> = vec![Vec::new(); line_count];

    for hl in &features.highlights {
        let kind = highlight_kind_to_token(hl.kind);
        let start = hl.range.start().to_usize();
        let end = hl.range.end().to_usize();
        if end <= start {
            continue;
        }

        let mut byte = start;
        let mut line_idx = line_starts
            .partition_point(|&ls| ls <= byte)
            .saturating_sub(1);
        while byte < end && line_idx < line_count {
            let line_start = line_starts[line_idx];
            let line_end = line_starts
                .get(line_idx + 1)
                .map(|&s| s.saturating_sub(1)) // exclude '\n'
                .unwrap_or(text.len());
            let frag_end = end.min(line_end);
            if frag_end > byte {
                result[line_idx].push(TokenSpan {
                    start: byte - line_start,
                    len: frag_end - byte,
                    kind,
                });
            }
            byte = line_starts
                .get(line_idx + 1)
                .copied()
                .unwrap_or(end)
                .max(frag_end);
            line_idx += 1;
        }
    }

    Some(result)
}

fn highlight_kind_to_token(kind: HighlightKind) -> TokenKind {
    match kind {
        HighlightKind::Keyword => TokenKind::Keyword,
        HighlightKind::KeywordControl => TokenKind::KeywordControl,
        HighlightKind::Type => TokenKind::Type,
        HighlightKind::String => TokenKind::String,
        HighlightKind::Comment => TokenKind::Comment,
        HighlightKind::Number => TokenKind::Number,
        HighlightKind::Operator => TokenKind::Operator,
        HighlightKind::Punctuation => TokenKind::Punctuation,
        HighlightKind::Function => TokenKind::Function,
        HighlightKind::Variable => TokenKind::Variable,
        HighlightKind::Attribute => TokenKind::Attribute,
        HighlightKind::Macro => TokenKind::Macro,
        HighlightKind::Lifetime => TokenKind::Lifetime,
        HighlightKind::SectionHeader => TokenKind::SectionHeader,
    }
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

    #[test]
    fn oxygen_keyword_and_string_highlighted() {
        let result = scan_dsl_buffer(Language::OXYGEN, r#"fn main() { let s = "hi"; }"#)
            .expect("oxygen plugin registered");
        let kinds: Vec<TokenKind> = result[0].iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::String));
    }

    #[test]
    fn multiline_block_comment_spans_both_lines() {
        let result = scan_dsl_buffer(Language::C, "/* start\nend */ int x;").unwrap();
        assert!(result[0].iter().any(|s| s.kind == TokenKind::Comment));
        assert!(result[1].iter().any(|s| s.kind == TokenKind::Comment));
    }

    #[test]
    fn unregistered_language_returns_none() {
        assert!(scan_dsl_buffer(Language::DOCKERFILE, "FROM rust").is_none());
    }
}
