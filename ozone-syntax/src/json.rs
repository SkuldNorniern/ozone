use super::{ScanState, TokenKind, TokenSpan};

pub(super) fn scan_json(line: &str, mut state: ScanState) -> (Vec<TokenSpan>, ScanState) {
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
