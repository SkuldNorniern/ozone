//! Syntax highlighting — Layer 0: deterministic line scanners.
//!
//! Each scanner is a pure function: (line_text, ScanState) → (Vec<TokenSpan>, ScanState).
//! No regex. No parser. Always produces a result; never panics or errors.
//!
//! Phase 1 covers Rust, TOML, JSON, and Markdown. More languages come later.

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
        Filetype::Rust => scan_rust(line, state),
        Filetype::Toml => (scan_toml(line), ScanState::clean()),
        Filetype::Json => scan_json(line, state),
        Filetype::Markdown => scan_markdown(line, state),
        Filetype::Plain => (vec![], ScanState::clean()),
    }
}

// ---------------------------------------------------------------------------
// Rust scanner
// ---------------------------------------------------------------------------

fn scan_rust(line: &str, mut state: ScanState) -> (Vec<TokenSpan>, ScanState) {
    let b = line.as_bytes();
    let n = b.len();
    let mut spans: Vec<TokenSpan> = Vec::new();
    let mut i = 0;

    // Helper: push span if non-empty
    macro_rules! push {
        ($start:expr, $end:expr, $kind:expr) => {
            if $end > $start {
                spans.push(TokenSpan {
                    start: $start,
                    len: $end - $start,
                    kind: $kind,
                });
            }
        };
    }

    // --- continuing block comment ---
    if state.block_comment_depth > 0 {
        let start = 0;
        while i < n {
            if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
                state.block_comment_depth += 1;
                i += 2;
            } else if i + 1 < n && b[i] == b'*' && b[i + 1] == b'/' {
                state.block_comment_depth -= 1;
                i += 2;
                if state.block_comment_depth == 0 {
                    push!(start, i, TokenKind::Comment);
                    break;
                }
            } else {
                i += 1;
            }
        }
        if state.block_comment_depth > 0 {
            push!(0, n, TokenKind::Comment);
            return (spans, state);
        }
    }

    while i < n {
        // --- line comment ---
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'/' {
            push!(i, n, TokenKind::Comment);
            break;
        }

        // --- block comment open ---
        if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
            let start = i;
            state.block_comment_depth += 1;
            i += 2;
            while i < n {
                if i + 1 < n && b[i] == b'/' && b[i + 1] == b'*' {
                    state.block_comment_depth += 1;
                    i += 2;
                } else if i + 1 < n && b[i] == b'*' && b[i + 1] == b'/' {
                    state.block_comment_depth -= 1;
                    i += 2;
                    if state.block_comment_depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            push!(start, i, TokenKind::Comment);
            continue;
        }

        // --- attribute ---
        if b[i] == b'#'
            && i + 1 < n
            && (b[i + 1] == b'[' || (b[i + 1] == b'!' && i + 2 < n && b[i + 2] == b'['))
        {
            let start = i;
            let mut depth = 0usize;
            while i < n {
                if b[i] == b'[' {
                    depth += 1;
                } else if b[i] == b']' {
                    depth -= 1;
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                i += 1;
            }
            push!(start, i, TokenKind::Attribute);
            continue;
        }

        // --- string literal (double-quoted) ---
        if b[i] == b'"' {
            let start = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                } else if b[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            push!(start, i, TokenKind::String);
            continue;
        }

        // --- byte string b"..." ---
        if b[i] == b'b' && i + 1 < n && b[i + 1] == b'"' {
            let start = i;
            i += 2;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                } else if b[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            push!(start, i, TokenKind::String);
            continue;
        }

        // --- char literal OR lifetime ---
        if b[i] == b'\'' {
            let start = i;
            i += 1;
            if i < n {
                if b[i] == b'\\' {
                    // Escape char literal: '\n', '\t', '\\', '\'' etc.
                    i += 1;
                    while i < n && b[i] != b'\'' {
                        i += 1;
                    }
                    if i < n {
                        i += 1;
                    }
                    push!(start, i, TokenKind::String);
                } else if i + 1 < n && b[i + 1] == b'\'' {
                    // Single-char literal 'x'
                    i += 2;
                    push!(start, i, TokenKind::String);
                } else if (b[i] as char).is_ascii_alphabetic() || b[i] == b'_' {
                    // Lifetime 'a or 'static
                    while i < n && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                        i += 1;
                    }
                    // Only a lifetime if NOT followed by '
                    if i >= n || b[i] != b'\'' {
                        push!(start, i, TokenKind::Lifetime);
                    } else {
                        // It was actually 'abc' — impossible for single-char handled above,
                        // but handle multi-char literals like 'AB' as string
                        i += 1;
                        push!(start, i, TokenKind::String);
                    }
                } else {
                    // Non-ascii char literal or something odd — skip
                    while i < n && b[i] != b'\'' {
                        i += 1;
                    }
                    if i < n {
                        i += 1;
                    }
                    push!(start, i, TokenKind::String);
                }
            }
            continue;
        }

        // --- number ---
        if (b[i] as char).is_ascii_digit()
            || (b[i] == b'-'
                && i + 1 < n
                && (b[i + 1] as char).is_ascii_digit()
                && (i == 0 || !((b[i - 1] as char).is_ascii_alphanumeric() || b[i - 1] == b')')))
        {
            let start = i;
            if b[i] == b'-' {
                i += 1;
            }
            // Hex
            if i + 1 < n && b[i] == b'0' && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                i += 2;
                while i < n && (b[i] as char).is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < n
                    && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.')
                {
                    i += 1;
                }
            }
            push!(start, i, TokenKind::Number);
            continue;
        }

        // --- identifier / keyword / type / macro / function ---
        if (b[i] as char).is_ascii_alphabetic() || b[i] == b'_' {
            let start = i;
            while i < n && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &line[start..i];

            // Macro call: word!(
            if i < n && b[i] == b'!' {
                i += 1;
                push!(start, i, TokenKind::Macro);
                continue;
            }

            // Function call or definition: word(
            let is_fn_call = i < n && b[i] == b'(';

            let kind = rust_keyword_kind(word, is_fn_call);
            push!(start, i, kind);
            continue;
        }

        // --- operators (colour a few) ---
        if matches!(
            b[i],
            b'+' | b'-'
                | b'*'
                | b'/'
                | b'%'
                | b'='
                | b'!'
                | b'<'
                | b'>'
                | b'&'
                | b'|'
                | b'^'
                | b'~'
        ) {
            push!(i, i + 1, TokenKind::Operator);
            i += 1;
            continue;
        }

        i += 1;
    }

    (spans, state)
}

fn rust_keyword_kind(word: &str, is_fn_call: bool) -> TokenKind {
    match word {
        // control-flow keywords
        "if" | "else" | "match" | "for" | "while" | "loop" | "break" | "continue" | "return"
        | "yield" => TokenKind::KeywordControl,

        // declaration keywords
        "fn" | "let" | "mut" | "const" | "static" | "struct" | "enum" | "union" | "trait"
        | "impl" | "type" | "where" | "pub" | "use" | "mod" | "extern" | "crate" | "super"
        | "self" | "Self" | "in" | "as" | "move" | "async" | "await" | "dyn" | "unsafe" | "ref"
        | "box" => TokenKind::Keyword,

        // primitive types
        "bool" | "char" | "str" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8"
        | "u16" | "u32" | "u64" | "u128" | "usize" | "f32" | "f64" => TokenKind::Type,

        // common std types (uppercase)
        "String" | "Vec" | "Option" | "Result" | "Box" | "Rc" | "Arc" | "HashMap" | "HashSet"
        | "BTreeMap" | "BTreeSet" | "Mutex" | "RwLock" | "PathBuf" | "Path" | "Cow" | "Pin"
        | "Error" => TokenKind::Type,

        // boolean literals
        "true" | "false" => TokenKind::Number,

        _ => {
            if is_fn_call {
                TokenKind::Function
            } else if word.len() > 1
                && word
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                // PascalCase → type/struct name
                TokenKind::Type
            } else {
                TokenKind::Variable
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TOML scanner
// ---------------------------------------------------------------------------

fn scan_toml(line: &str) -> Vec<TokenSpan> {
    let b = line.as_bytes();
    let n = b.len();
    let mut spans = Vec::new();
    let mut i = 0;

    macro_rules! push {
        ($start:expr, $end:expr, $kind:expr) => {
            if $end > $start {
                spans.push(TokenSpan {
                    start: $start,
                    len: $end - $start,
                    kind: $kind,
                });
            }
        };
    }

    // Skip leading whitespace
    while i < n && b[i] == b' ' {
        i += 1;
    }

    // Comment
    if i < n && b[i] == b'#' {
        push!(i, n, TokenKind::Comment);
        return spans;
    }

    // Section header [section] or [[array]]
    if i < n && b[i] == b'[' {
        push!(i, n, TokenKind::SectionHeader);
        return spans;
    }

    // Key = value
    // Key (up to =)
    let key_start = i;
    while i < n && b[i] != b'=' && b[i] != b'#' {
        i += 1;
    }
    let key_end = i;

    if i < n && b[i] == b'=' {
        push!(key_start, key_end, TokenKind::Keyword); // key
        i += 1; // skip =

        // Skip whitespace before value
        while i < n && b[i] == b' ' {
            i += 1;
        }

        if i >= n {
            return spans;
        }

        // Value
        let val_start = i;

        // Comment after value
        let val_end = {
            let mut j = n;
            // Find trailing comment (not inside strings)
            let mut in_str = false;
            let mut str_ch = b'"';
            let mut k = i;
            while k < n {
                if !in_str && (b[k] == b'"' || b[k] == b'\'') {
                    in_str = true;
                    str_ch = b[k];
                    k += 1;
                } else if in_str && b[k] == str_ch {
                    in_str = false;
                    k += 1;
                } else if !in_str && b[k] == b'#' {
                    j = k;
                    break;
                } else {
                    k += 1;
                }
            }
            j
        };

        let val = &line[val_start..val_end].trim_end();

        let is_number = *val == "true"
            || *val == "false"
            || val
                .chars()
                .next()
                .map(|c| c.is_ascii_digit() || c == '-')
                .unwrap_or(false);
        let kind = if val.starts_with('"') || val.starts_with('\'') {
            TokenKind::String
        } else if val.starts_with('[') || val.starts_with('{') {
            TokenKind::Punctuation
        } else if is_number {
            TokenKind::Number
        } else {
            TokenKind::Variable
        };

        push!(val_start, val_end, kind);

        // Trailing comment
        if val_end < n {
            push!(val_end, n, TokenKind::Comment);
        }
    }

    spans
}

// ---------------------------------------------------------------------------
// JSON scanner (tolerates JSONC `//` and `/* */` comments)
// ---------------------------------------------------------------------------

fn scan_json(line: &str, mut state: ScanState) -> (Vec<TokenSpan>, ScanState) {
    let b = line.as_bytes();
    let n = b.len();
    let mut spans: Vec<TokenSpan> = Vec::new();
    let mut i = 0;

    macro_rules! push {
        ($start:expr, $end:expr, $kind:expr) => {
            if $end > $start {
                spans.push(TokenSpan {
                    start: $start,
                    len: $end - $start,
                    kind: $kind,
                });
            }
        };
    }

    // --- continuing /* */ block comment ---
    if state.block_comment_depth > 0 {
        while i < n {
            if i + 1 < n && b[i] == b'*' && b[i + 1] == b'/' {
                i += 2;
                state.block_comment_depth = 0;
                push!(0, i, TokenKind::Comment);
                break;
            }
            i += 1;
        }
        if state.block_comment_depth > 0 {
            push!(0, n, TokenKind::Comment);
            return (spans, state);
        }
    }

    while i < n {
        let c = b[i];

        // line comment
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            push!(i, n, TokenKind::Comment);
            break;
        }
        // block comment open
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let start = i;
            i += 2;
            let mut closed = false;
            while i < n {
                if i + 1 < n && b[i] == b'*' && b[i + 1] == b'/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                state.block_comment_depth = 1;
                push!(start, n, TokenKind::Comment);
                return (spans, state);
            }
            push!(start, i, TokenKind::Comment);
            continue;
        }

        // string — object key if the next non-space char is ':'
        if c == b'"' {
            let start = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                } else if b[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            let end = i;
            let mut j = end;
            while j < n && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            let kind = if j < n && b[j] == b':' {
                TokenKind::Keyword
            } else {
                TokenKind::String
            };
            push!(start, end, kind);
            continue;
        }

        // number
        if (c as char).is_ascii_digit()
            || (c == b'-' && i + 1 < n && (b[i + 1] as char).is_ascii_digit())
        {
            let start = i;
            if c == b'-' {
                i += 1;
            }
            while i < n
                && ((b[i] as char).is_ascii_alphanumeric()
                    || b[i] == b'.'
                    || b[i] == b'+'
                    || b[i] == b'-')
            {
                i += 1;
            }
            push!(start, i, TokenKind::Number);
            continue;
        }

        // literals true / false / null
        if (c as char).is_ascii_alphabetic() {
            let start = i;
            while i < n && (b[i] as char).is_ascii_alphabetic() {
                i += 1;
            }
            let kind = match &line[start..i] {
                "true" | "false" => TokenKind::Number,
                "null" => TokenKind::Keyword,
                _ => TokenKind::Variable,
            };
            push!(start, i, kind);
            continue;
        }

        // structural punctuation
        if matches!(c, b'{' | b'}' | b'[' | b']' | b',' | b':') {
            push!(i, i + 1, TokenKind::Punctuation);
            i += 1;
            continue;
        }

        i += 1;
    }

    (spans, state)
}

// ---------------------------------------------------------------------------
// Markdown scanner
// ---------------------------------------------------------------------------

fn scan_markdown(line: &str, mut state: ScanState) -> (Vec<TokenSpan>, ScanState) {
    let b = line.as_bytes();
    let n = b.len();
    let mut spans: Vec<TokenSpan> = Vec::new();

    macro_rules! push {
        ($start:expr, $end:expr, $kind:expr) => {
            if $end > $start {
                spans.push(TokenSpan {
                    start: $start,
                    len: $end - $start,
                    kind: $kind,
                });
            }
        };
    }

    // Leading indentation.
    let mut ts = 0;
    while ts < n && (b[ts] == b' ' || b[ts] == b'\t') {
        ts += 1;
    }

    // Fenced code block delimiter: 3+ of ` or ~.
    let is_fence = ts < n
        && (b[ts] == b'`' || b[ts] == b'~')
        && ts + 2 < n
        && b[ts + 1] == b[ts]
        && b[ts + 2] == b[ts];

    if state.in_code_fence {
        if is_fence {
            state.in_code_fence = false;
            push!(0, n, TokenKind::Comment);
        }
        // Code content inside the fence stays Default (no spans).
        return (spans, state);
    }
    if is_fence {
        state.in_code_fence = true;
        push!(0, n, TokenKind::Comment);
        return (spans, state);
    }

    // ATX heading: 1–6 '#' then space or end of line → whole line highlighted.
    if ts < n && b[ts] == b'#' {
        let mut h = ts;
        while h < n && b[h] == b'#' {
            h += 1;
        }
        let level = h - ts;
        if (1..=6).contains(&level) && (h >= n || b[h] == b' ') {
            push!(0, n, TokenKind::Keyword);
            return (spans, state);
        }
    }

    // Blockquote.
    if ts < n && b[ts] == b'>' {
        push!(0, n, TokenKind::Comment);
        return (spans, state);
    }

    // List marker (-, *, + or "N." / "N)") then a space.
    let mut i = ts;
    if ts < n {
        if matches!(b[ts], b'-' | b'*' | b'+') && ts + 1 < n && b[ts + 1] == b' ' {
            push!(ts, ts + 1, TokenKind::Operator);
            i = ts + 1;
        } else if (b[ts] as char).is_ascii_digit() {
            let mut k = ts;
            while k < n && (b[k] as char).is_ascii_digit() {
                k += 1;
            }
            if k < n && (b[k] == b'.' || b[k] == b')') && k + 1 < n && b[k + 1] == b' ' {
                push!(ts, k + 1, TokenKind::Operator);
                i = k + 1;
            }
        }
    }

    // Inline scan: code spans `…` and links [text](url).
    while i < n {
        let c = b[i];

        if c == b'`' {
            let start = i;
            i += 1;
            while i < n && b[i] != b'`' {
                i += 1;
            }
            if i < n {
                i += 1; // include closing backtick
            }
            push!(start, i, TokenKind::String);
            continue;
        }

        if c == b'[' {
            let mut k = i + 1;
            while k < n && b[k] != b']' {
                k += 1;
            }
            if k + 1 < n && b[k] == b']' && b[k + 1] == b'(' {
                let mut u = k + 2;
                while u < n && b[u] != b')' {
                    u += 1;
                }
                if u < n {
                    push!(i, k + 1, TokenKind::Function); // [text]
                    push!(k + 1, u + 1, TokenKind::String); // (url)
                    i = u + 1;
                    continue;
                }
            }
        }

        i += 1;
    }

    (spans, state)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
