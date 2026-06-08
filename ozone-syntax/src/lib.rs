//! Syntax highlighting — Layer 0: deterministic line scanners.
//!
//! Each scanner is a pure function: (line_text, ScanState) → (Vec<TokenSpan>, ScanState).
//! No regex. No parser. Always produces a result; never panics or errors.
//!
//! Phase 1 covers Rust, TOML, JSON, and Markdown. More languages come later.

mod json;
mod markdown;
mod rust;
mod toml;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Byte-level token span over one line (offsets relative to line start).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSpan {
    pub start: usize,
    pub len: usize,
    pub kind: TokenKind,
}

/// High-level token category used for colour mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Default,
    Keyword,
    KeywordControl,
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
    Lifetime,
    SectionHeader, // TOML [section]
}

// ---------------------------------------------------------------------------
// Filetype
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filetype {
    Rust,
    Toml,
    Json,
    Markdown,
    Plain,
}

impl Filetype {
    pub fn from_path(path: &str) -> Self {
        // Match on the lowercased final extension.
        let ext = path.rsplit('.').next().unwrap_or("");
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Filetype::Rust,
            "toml" => Filetype::Toml,
            "json" | "jsonc" => Filetype::Json,
            "md" | "markdown" | "mdown" | "mkd" => Filetype::Markdown,
            _ => Filetype::Plain,
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-line scan state
// ---------------------------------------------------------------------------

/// State carried across lines for multi-line constructs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanState {
    /// Depth of nested `/* */` block comments (Rust allows nesting).
    pub block_comment_depth: u32,
    /// Inside a multi-line raw string (simplified: just track open/close).
    pub in_raw_string: bool,
    /// Inside a Markdown fenced code block (``` / ~~~).
    pub in_code_fence: bool,
}

impl ScanState {
    pub fn clean() -> Self {
        Self::default()
    }

    pub fn is_clean(self) -> bool {
        self.block_comment_depth == 0 && !self.in_raw_string && !self.in_code_fence
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Scan one line. Returns token spans and the updated scan state.
/// Spans cover only the "interesting" (coloured) regions; gaps are Default.
pub fn scan_line(ft: Filetype, line: &str, state: ScanState) -> (Vec<TokenSpan>, ScanState) {
    match ft {
        Filetype::Rust => rust::scan_rust(line, state),
        Filetype::Toml => (toml::scan_toml(line), ScanState::clean()),
        Filetype::Json => json::scan_json(line, state),
        Filetype::Markdown => markdown::scan_markdown(line, state),
        Filetype::Plain => (vec![], ScanState::clean()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::json::scan_json;
    use super::markdown::scan_markdown;

    fn kinds(spans: &[TokenSpan]) -> Vec<TokenKind> {
        spans.iter().map(|s| s.kind).collect()
    }

    #[test]
    fn filetype_detection() {
        assert_eq!(Filetype::from_path("a.json"), Filetype::Json);
        assert_eq!(Filetype::from_path("README.md"), Filetype::Markdown);
        assert_eq!(Filetype::from_path("X.MD"), Filetype::Markdown);
        assert_eq!(Filetype::from_path("c.rs"), Filetype::Rust);
        assert_eq!(Filetype::from_path("noext"), Filetype::Plain);
    }

    #[test]
    fn json_key_vs_string_value() {
        let (spans, st) = scan_json(r#"  "name": "ozone","#, ScanState::clean());
        assert!(st.is_clean());
        // first string is a key, second is a value
        assert_eq!(spans[0].kind, TokenKind::Keyword);
        assert!(spans.iter().any(|s| s.kind == TokenKind::String));
        assert!(spans.iter().any(|s| s.kind == TokenKind::Punctuation));
    }

    #[test]
    fn json_numbers_and_literals() {
        let (spans, _) = scan_json(r#"{ "n": 42, "ok": true, "x": null }"#, ScanState::clean());
        assert!(spans.iter().any(|s| s.kind == TokenKind::Number));
        assert!(spans.iter().any(|s| s.kind == TokenKind::Keyword)); // key + null
    }

    #[test]
    fn json_multiline_block_comment() {
        let (_s1, st1) = scan_json("/* start", ScanState::clean());
        assert!(!st1.is_clean());
        let (s2, st2) = scan_json("still */ 1", st1);
        assert!(st2.is_clean());
        assert_eq!(s2[0].kind, TokenKind::Comment);
        assert!(s2.iter().any(|s| s.kind == TokenKind::Number));
    }

    #[test]
    fn markdown_heading_and_quote() {
        let (h, _) = scan_markdown("## Title", ScanState::clean());
        assert_eq!(kinds(&h), vec![TokenKind::Keyword]);
        let (q, _) = scan_markdown("> quoted", ScanState::clean());
        assert_eq!(kinds(&q), vec![TokenKind::Comment]);
        // '#' without trailing space is not a heading
        let (nh, _) = scan_markdown("#tag", ScanState::clean());
        assert!(nh.is_empty());
    }

    #[test]
    fn markdown_fenced_code_block() {
        let (f1, st1) = scan_markdown("```rust", ScanState::clean());
        assert_eq!(f1[0].kind, TokenKind::Comment);
        assert!(st1.in_code_fence);
        let (code, st2) = scan_markdown("let x = 1;", st1);
        assert!(code.is_empty()); // code content left Default
        assert!(st2.in_code_fence);
        let (f2, st3) = scan_markdown("```", st2);
        assert_eq!(f2[0].kind, TokenKind::Comment);
        assert!(st3.is_clean());
    }

    #[test]
    fn markdown_list_inline_code_and_link() {
        let (spans, _) = scan_markdown("- see `code` and [text](http://x)", ScanState::clean());
        assert_eq!(spans[0].kind, TokenKind::Operator); // list marker
        assert!(spans.iter().any(|s| s.kind == TokenKind::String)); // inline code + url
        assert!(spans.iter().any(|s| s.kind == TokenKind::Function)); // [text]
    }
}
