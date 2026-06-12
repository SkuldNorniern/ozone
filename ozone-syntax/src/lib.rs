//! Syntax highlighting — Layer 0: deterministic line scanners.
//!
//! Each scanner is a pure function: (line_text, ScanState) → (Vec<TokenSpan>, ScanState).
//! No regex. No parser. Always produces a result; never panics or errors.
//!
//! Phase 1 covers Rust, TOML, JSON, and Markdown. More languages come later.

mod dsl_buffer;
mod json;
mod json_buffer;
mod markdown;
mod markdown_buffer;
mod rust;
mod rust_buffer;
pub mod symbols;
mod toml;
mod toml_buffer;
mod yaml_buffer;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use sylven::lang::rust::RustLanguage;
use sylven::{DocumentId, LanguageId, RevisionId, SyntaxEngine, TextSnapshot};
use taste::Language;

pub use sylven::SyntaxFeatures;
pub use symbols::{Symbol, SymbolKind, symbols};

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
///
/// For Rust, prefer [`scan_buffer`] — it uses the full-document sylven lexer
/// which handles nested block comments and raw strings across lines correctly.
pub fn scan_line(
    lang: Option<Language>,
    line: &str,
    state: ScanState,
) -> (Vec<TokenSpan>, ScanState) {
    match lang {
        Some(Language::RUST) => rust::scan_rust(line, state),
        Some(Language::TOML) => (toml::scan_toml(line), ScanState::clean()),
        Some(Language::JSON) => json::scan_json(line, state),
        Some(Language::MARKDOWN) => markdown::scan_markdown(line, state),
        _ => (vec![], ScanState::clean()),
    }
}

/// Scan an entire document at once and return per-line token spans.
///
/// For Rust this uses the sylven full-document lexer, which handles nested
/// block comments and raw strings across line boundaries. Other languages fall
/// back to the line-by-line scanners.
///
/// The returned `Vec` has one entry per line (split on `\n`). Spans are
/// relative to the start of each line (byte offsets within the line string).
pub fn scan_buffer(lang: Option<Language>, text: &str) -> Vec<Vec<TokenSpan>> {
    match lang {
        Some(Language::RUST) => rust_buffer::scan_rust_buffer(text),
        Some(Language::TOML) => toml_buffer::scan_toml_buffer(text),
        Some(Language::MARKDOWN) => markdown_buffer::scan_markdown_buffer(text),
        Some(Language::JSON) => json_buffer::scan_json_buffer(text),
        Some(Language::YAML) => yaml_buffer::scan_yaml_buffer(text),
        Some(other) => {
            dsl_buffer::scan_dsl_buffer(other, text).unwrap_or_else(|| line_by_line(lang, text))
        }
        None => line_by_line(lang, text),
    }
}

/// Fallback for languages with neither a dedicated buffer scanner nor a
/// registered sylven plugin: scan line by line via [`scan_line`].
fn line_by_line(lang: Option<Language>, text: &str) -> Vec<Vec<TokenSpan>> {
    let mut result = Vec::new();
    let mut state = ScanState::clean();
    for line in text.split('\n') {
        let (spans, new_state) = scan_line(lang, line, state);
        state = new_state;
        result.push(spans);
    }
    result
}

// ---------------------------------------------------------------------------
// Structural parsing via sylven
// ---------------------------------------------------------------------------

static ENGINE: OnceLock<SyntaxEngine> = OnceLock::new();

fn engine() -> &'static SyntaxEngine {
    ENGINE.get_or_init(|| {
        let mut e = SyntaxEngine::new();
        // Add DSL-based plugins (C, Python, Oxygen); DSL Rust also registers here
        sylven_runtime::register_bundled(e.registry_mut());
        // Re-register hand-written Rust: it has symbol extraction the DSL version lacks
        e.registry_mut().register(Arc::new(RustLanguage));
        e
    })
}

/// Expand `(sel_start, sel_end)` byte offsets to the smallest structural range
/// strictly containing them, using sylven bracket and fold data. Returns
/// `(new_start, new_end)` byte offsets, or `None` when already at the outermost
/// range or the language has no sylven plugin.
pub fn expand_selection(
    lang: Option<Language>,
    text: &str,
    sel_start: usize,
    sel_end: usize,
) -> Option<(usize, usize)> {
    let features = parse_features(lang, text)?;
    sylven::expand_selection(&features, text.len(), sel_start, sel_end)
}

/// One remembered parse: language + content fingerprint → features.
/// `text` is kept to verify a hash match (collisions must not corrupt
/// highlighting), and `Arc`d so verification doesn't copy the buffer.
struct ParseMemo {
    lang: &'static str,
    hash: u64,
    text: Arc<str>,
    features: Arc<SyntaxFeatures>,
}

/// Content-addressed memo over recent parses, most-recent first.
///
/// Every structural consumer funnels through [`parse_features`]: render
/// highlights and folds, fold commands, expand-selection, the symbol picker,
/// and fold-gutter mouse clicks. Without the memo each of those costs a full
/// document parse; with it, one buffer revision is parsed once and every
/// other query is a hash + compare.
static PARSE_MEMO: Mutex<Vec<ParseMemo>> = Mutex::new(Vec::new());
const PARSE_MEMO_CAP: usize = 8;

fn text_hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Parse structural features (highlights, folds, symbols, brackets) for
/// a buffer via sylven. Returns `None` for languages without a registered
/// plugin (e.g. unknown / plain text).
///
/// Results are memoized on (language, content), so calling this repeatedly
/// for the same buffer revision is cheap.
pub fn parse_features(lang: Option<Language>, text: &str) -> Option<Arc<SyntaxFeatures>> {
    let name = lang?.name();
    let hash = text_hash(text);

    let mut memo = PARSE_MEMO.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = memo
        .iter()
        .position(|m| m.lang == name && m.hash == hash && *m.text == *text)
    {
        let entry = memo.remove(i);
        let features = Arc::clone(&entry.features);
        memo.insert(0, entry); // refresh LRU position
        return Some(features);
    }
    drop(memo); // don't hold the lock across a potentially slow parse

    let snapshot = TextSnapshot::new(DocumentId(0), RevisionId(0), text);
    let features = Arc::new(engine().parse(LanguageId(name), &snapshot)?.features);

    let mut memo = PARSE_MEMO.lock().unwrap_or_else(|e| e.into_inner());
    memo.insert(
        0,
        ParseMemo {
            lang: name,
            hash,
            text: Arc::from(text),
            features: Arc::clone(&features),
        },
    );
    memo.truncate(PARSE_MEMO_CAP);
    Some(features)
}

/// Return structural fold ranges as `(start_line, end_line)` pairs (inclusive,
/// 0-based). Always spans at least two lines. Returns an empty `Vec` for
/// languages without a sylven plugin.
pub fn fold_line_ranges(lang: Option<Language>, text: &str) -> Vec<(usize, usize)> {
    let Some(features) = parse_features(lang, text) else {
        return Vec::new();
    };
    features
        .folds
        .iter()
        .map(|r| {
            let start = byte_to_line(text, r.start().to_usize());
            let end = byte_to_line(text, r.end().to_usize().saturating_sub(1));
            (start, end)
        })
        .filter(|&(s, e)| e > s)
        .collect()
}

pub(crate) fn byte_to_line(text: &str, offset: usize) -> usize {
    let safe = offset.min(text.len());
    text[..safe].bytes().filter(|&b| b == b'\n').count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::json::scan_json;
    use super::markdown::scan_markdown;
    use super::*;
    use taste::detect_language;

    fn kinds(spans: &[TokenSpan]) -> Vec<TokenKind> {
        spans.iter().map(|s| s.kind).collect()
    }

    #[test]
    fn language_detection() {
        assert_eq!(detect_language("a.json"), Some(Language::JSON));
        assert_eq!(detect_language("README.md"), Some(Language::MARKDOWN));
        assert_eq!(detect_language("X.MD"), Some(Language::MARKDOWN));
        assert_eq!(detect_language("c.rs"), Some(Language::RUST));
        assert_eq!(detect_language("a.yaml"), Some(Language::YAML));
        assert_eq!(detect_language("a.yml"), Some(Language::YAML));
        assert_eq!(detect_language("noext"), None);
        assert_eq!(detect_language("plugin.oxy"), Some(Language::OXYGEN));
    }

    #[test]
    fn oxygen_highlights_via_bundled_plugin() {
        let lang = detect_language("plugin.oxy");
        let features = parse_features(lang, "fn main() {}").expect("oxygen plugin registered");
        assert!(
            features
                .highlights
                .iter()
                .any(|h| h.kind == sylven::HighlightKind::Keyword)
        );
    }

    #[test]
    fn json_key_vs_string_value() {
        let (spans, st) = scan_json(r#"  "name": "ozone","#, ScanState::clean());
        assert!(st.is_clean());
        assert_eq!(spans[0].kind, TokenKind::Keyword);
        assert!(spans.iter().any(|s| s.kind == TokenKind::String));
        assert!(spans.iter().any(|s| s.kind == TokenKind::Punctuation));
    }

    #[test]
    fn json_numbers_and_literals() {
        let (spans, _) = scan_json(r#"{ "n": 42, "ok": true, "x": null }"#, ScanState::clean());
        assert!(spans.iter().any(|s| s.kind == TokenKind::Number));
        assert!(spans.iter().any(|s| s.kind == TokenKind::Keyword));
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
        let (nh, _) = scan_markdown("#tag", ScanState::clean());
        assert!(nh.is_empty());
    }

    #[test]
    fn markdown_fenced_code_block() {
        let (f1, st1) = scan_markdown("```rust", ScanState::clean());
        assert_eq!(f1[0].kind, TokenKind::Comment);
        assert!(st1.in_code_fence);
        let (code, st2) = scan_markdown("let x = 1;", st1);
        assert!(code.is_empty());
        assert!(st2.in_code_fence);
        let (f2, st3) = scan_markdown("```", st2);
        assert_eq!(f2[0].kind, TokenKind::Comment);
        assert!(st3.is_clean());
    }

    #[test]
    fn markdown_list_inline_code_and_link() {
        let (spans, _) = scan_markdown("- see `code` and [text](http://x)", ScanState::clean());
        assert_eq!(spans[0].kind, TokenKind::Operator);
        assert!(spans.iter().any(|s| s.kind == TokenKind::String));
        assert!(spans.iter().any(|s| s.kind == TokenKind::Function));
    }
}
