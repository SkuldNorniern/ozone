//! Syntax highlighting — Phase 0 stub.
//!
//! Phase 1 will add deterministic Layer-0 scanners for Rust, Markdown, JSON, TOML.

/// A classified token span over a buffer line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSpan {
    pub start: usize,
    pub len: usize,
    pub kind: TokenKind,
}

/// High-level token categories shared by all layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Default,
    Keyword,
    Type,
    String,
    Comment,
    Number,
    Operator,
    Punctuation,
    Function,
    Variable,
    Attribute,
    Macro,
}

/// Layer-0 scanner interface (stub).
pub trait Scanner {
    /// Tokenise a single line. Returns token spans relative to line start.
    fn scan_line(&self, line: &str) -> Vec<TokenSpan>;
}

/// A scanner that emits one Default token covering the whole line.
pub struct PassthroughScanner;

impl Scanner for PassthroughScanner {
    fn scan_line(&self, line: &str) -> Vec<TokenSpan> {
        if line.is_empty() {
            vec![]
        } else {
            vec![TokenSpan { start: 0, len: line.len(), kind: TokenKind::Default }]
        }
    }
}
