use super::{TokenKind, TokenSpan};

pub(super) fn scan_toml(line: &str) -> Vec<TokenSpan> {
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
