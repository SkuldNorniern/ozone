//! Text objects: deterministic spans around a position — word, line, the
//! bracket pair enclosing the cursor, and the quotes around it.
//!
//! Layer-0 (no structural parser): word/quote objects are line-local and
//! byte-scanned; bracket objects depth-count over the whole buffer like
//! [`crate::brackets`]. These power selection commands (`select.word`,
//! `select.inside-brackets`, …) and are the building blocks a future structural
//! pass can refine. All spans are UTF-8-boundary safe.

use ozone_buffer::{Buffer, Pos, Span};

/// Bytes that belong to a "word": ASCII alphanumerics, `_`, and any non-ASCII
/// byte (so multi-byte identifiers stay intact without splitting a codepoint).
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// The word run covering (or immediately before) `pos` on its line. `None` when
/// the cursor is not on/after a word byte.
pub fn word_at(buf: &Buffer, pos: Pos) -> Option<Span> {
    let line = buf.line(pos.line)?;
    let bytes = line.as_bytes();
    let len = bytes.len();

    // Anchor on the word byte under the cursor, else the one just before it.
    let anchor = if pos.col < len && is_word(bytes[pos.col]) {
        pos.col
    } else if pos.col > 0 && pos.col - 1 < len && is_word(bytes[pos.col - 1]) {
        pos.col - 1
    } else {
        return None;
    };

    let mut start = anchor;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < len && is_word(bytes[end]) {
        end += 1;
    }
    Some(Span {
        start: Pos::new(pos.line, start),
        end: Pos::new(pos.line, end),
    })
}

/// The line's content without its trailing newline (the "inner line" object).
pub fn line_inner(buf: &Buffer, pos: Pos) -> Span {
    let len = buf.line_len(pos.line);
    Span {
        start: Pos::new(pos.line, 0),
        end: Pos::new(pos.line, len),
    }
}

/// The whole line including its trailing newline, i.e. up to the start of the
/// next line (the "a line" object). On the last line this equals `line_inner`.
pub fn line_outer(buf: &Buffer, pos: Pos) -> Span {
    if pos.line + 1 < buf.line_count() {
        Span {
            start: Pos::new(pos.line, 0),
            end: Pos::new(pos.line + 1, 0),
        }
    } else {
        line_inner(buf, pos)
    }
}

/// The innermost bracket pair enclosing `pos` as `(open_offset, close_offset)`.
fn enclosing_pair(buf: &Buffer, pos: Pos) -> Option<(usize, usize)> {
    let off = buf.pos_to_offset(pos);
    buf.with_text(|text| {
        let tb = text.as_bytes();
        let mut stack: Vec<(usize, u8)> = Vec::new();
        let mut best: Option<(usize, usize)> = None;
        for (i, &c) in tb.iter().enumerate() {
            match c {
                b'(' | b'[' | b'{' => stack.push((i, c)),
                b')' | b']' | b'}' => {
                    let want = match c {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    if let Some((o, k)) = stack.last().copied()
                        && k == want
                    {
                        stack.pop();
                        // Enclosing if the cursor sits within [open, close].
                        if o <= off && off <= i {
                            match best {
                                Some((bo, _)) if bo >= o => {}
                                _ => best = Some((o, i)),
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        best
    })
}

/// Span between the brackets enclosing `pos`, excluding the brackets.
pub fn inside_brackets(buf: &Buffer, pos: Pos) -> Option<Span> {
    let (o, c) = enclosing_pair(buf, pos)?;
    Some(Span {
        start: buf.offset_to_pos(o + 1),
        end: buf.offset_to_pos(c),
    })
}

/// Span covering the brackets enclosing `pos`, including the brackets.
pub fn around_brackets(buf: &Buffer, pos: Pos) -> Option<Span> {
    let (o, c) = enclosing_pair(buf, pos)?;
    Some(Span {
        start: buf.offset_to_pos(o),
        end: buf.offset_to_pos(c + 1),
    })
}

/// Span inside the pair of quotes (`"` or `'`) surrounding `pos` on its line.
pub fn inside_quotes(buf: &Buffer, pos: Pos) -> Option<Span> {
    let line = buf.line(pos.line)?;
    let bytes = line.as_bytes();
    for quote in [b'"', b'\''] {
        let idx: Vec<usize> = bytes
            .iter()
            .enumerate()
            .filter_map(|(i, &b)| (b == quote).then_some(i))
            .collect();
        let mut k = 0;
        while k + 1 < idx.len() {
            let (a, b) = (idx[k], idx[k + 1]);
            if a < pos.col && pos.col <= b {
                return Some(Span {
                    start: Pos::new(pos.line, a + 1),
                    end: Pos::new(pos.line, b),
                });
            }
            k += 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::from_text(s)
    }

    #[test]
    fn word_under_and_after_cursor() {
        let b = buf("foo bar_baz qux");
        let w = word_at(&b, Pos::new(0, 5)).unwrap(); // inside "bar_baz"
        assert_eq!(w.start, Pos::new(0, 4));
        assert_eq!(w.end, Pos::new(0, 11));
        // not on a word byte (the space at col 3) → uses preceding word "foo"
        let w2 = word_at(&b, Pos::new(0, 3)).unwrap();
        assert_eq!((w2.start.col, w2.end.col), (0, 3));
    }

    #[test]
    fn word_none_in_whitespace() {
        let b = buf("   x");
        assert!(word_at(&b, Pos::new(0, 0)).is_none());
    }

    #[test]
    fn line_objects() {
        let b = buf("alpha\nbravo");
        assert_eq!(line_inner(&b, Pos::new(0, 2)).end, Pos::new(0, 5));
        assert_eq!(line_outer(&b, Pos::new(0, 2)).end, Pos::new(1, 0));
        // last line: outer == inner
        assert_eq!(line_outer(&b, Pos::new(1, 0)).end, Pos::new(1, 5));
    }

    #[test]
    fn inside_and_around_brackets() {
        let b = buf("foo(bar, baz)");
        let inner = inside_brackets(&b, Pos::new(0, 6)).unwrap();
        assert_eq!(inner.start, Pos::new(0, 4));
        assert_eq!(inner.end, Pos::new(0, 12));
        let around = around_brackets(&b, Pos::new(0, 6)).unwrap();
        assert_eq!(around.start, Pos::new(0, 3));
        assert_eq!(around.end, Pos::new(0, 13));
    }

    #[test]
    fn innermost_bracket_pair_wins() {
        let b = buf("a(b[c]d)e");
        // cursor inside the [] → innermost is the square pair
        let inner = inside_brackets(&b, Pos::new(0, 4)).unwrap();
        assert_eq!(inner.start, Pos::new(0, 4));
        assert_eq!(inner.end, Pos::new(0, 5));
    }

    #[test]
    fn brackets_across_lines() {
        let b = buf("fn x() {\n    body\n}");
        let inner = inside_brackets(&b, Pos::new(1, 4)).unwrap();
        assert_eq!(inner.start, Pos::new(0, 8)); // just after '{'
        assert_eq!(inner.end, Pos::new(2, 0)); // the closing '}'
    }

    #[test]
    fn quotes_on_line() {
        let b = buf("let s = \"hello world\";");
        let inner = inside_quotes(&b, Pos::new(0, 12)).unwrap();
        assert_eq!(inner.start, Pos::new(0, 9));
        assert_eq!(inner.end, Pos::new(0, 20));
    }

    #[test]
    fn no_enclosing_bracket() {
        let b = buf("plain text");
        assert!(inside_brackets(&b, Pos::new(0, 3)).is_none());
    }
}
