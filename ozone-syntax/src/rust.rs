use super::{ScanState, TokenKind, TokenSpan};

pub(super) fn scan_rust(line: &str, mut state: ScanState) -> (Vec<TokenSpan>, ScanState) {
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

pub(super) fn rust_keyword_kind(word: &str, is_fn_call: bool) -> TokenKind {
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
