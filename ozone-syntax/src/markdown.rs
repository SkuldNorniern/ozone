use super::{ScanState, TokenKind, TokenSpan};

pub(super) fn scan_markdown(line: &str, mut state: ScanState) -> (Vec<TokenSpan>, ScanState) {
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
