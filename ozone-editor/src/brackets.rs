//! Matching-bracket lookup for the cursor.
//!
//! Given a buffer and cursor, if the cursor sits on (or just after) a bracket,
//! find its partner by depth-counting. String/comment awareness is left to a
//! later structural pass — this is the deterministic, always-on version.

use ozone_buffer::{Buffer, Pos};

fn is_bracket(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'[' | b']' | b'{' | b'}')
}

/// If the cursor is on or immediately after a bracket, return both the bracket's
/// position and its matching partner's position. `None` if there's no bracket at
/// the cursor or no balanced match.
pub fn matching_bracket(buf: &Buffer, cursor: Pos) -> Option<(Pos, Pos)> {
    let line = buf.line(cursor.line)?;
    let bytes = line.as_bytes();

    // Prefer the bracket under the cursor, then the one just before it.
    let (bracket_pos, bracket) = if cursor.col < bytes.len() && is_bracket(bytes[cursor.col]) {
        (cursor, bytes[cursor.col])
    } else if cursor.col > 0 && cursor.col - 1 < bytes.len() && is_bracket(bytes[cursor.col - 1]) {
        (Pos::new(cursor.line, cursor.col - 1), bytes[cursor.col - 1])
    } else {
        return None;
    };

    let (open, close, forward) = match bracket {
        b'(' => (b'(', b')', true),
        b'[' => (b'[', b']', true),
        b'{' => (b'{', b'}', true),
        b')' => (b'(', b')', false),
        b']' => (b'[', b']', false),
        b'}' => (b'{', b'}', false),
        _ => return None,
    };

    let text = buf.text();
    let tb = text.as_bytes();
    let start = buf.pos_to_offset(bracket_pos);
    let mut depth = 0i32;

    if forward {
        let mut i = start;
        while i < tb.len() {
            let c = tb[i];
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some((bracket_pos, buf.offset_to_pos(i)));
                }
            }
            i += 1;
        }
    } else {
        let mut i = start as isize;
        while i >= 0 {
            let c = tb[i as usize];
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some((bracket_pos, buf.offset_to_pos(i as usize)));
                }
            }
            i -= 1;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(content: &str) -> Buffer {
        Buffer::from_text(content)
    }

    #[test]
    fn matches_forward_same_line() {
        let b = buf("foo(bar)");
        // cursor on '('
        let (a, m) = matching_bracket(&b, Pos::new(0, 3)).unwrap();
        assert_eq!(a, Pos::new(0, 3));
        assert_eq!(m, Pos::new(0, 7));
    }

    #[test]
    fn matches_backward_from_closer() {
        let b = buf("foo(bar)");
        // cursor on ')'
        let (a, m) = matching_bracket(&b, Pos::new(0, 7)).unwrap();
        assert_eq!(a, Pos::new(0, 7));
        assert_eq!(m, Pos::new(0, 3));
    }

    #[test]
    fn matches_just_after_bracket() {
        let b = buf("()");
        // cursor after ')' at col 2 -> uses the bracket before it
        let (a, m) = matching_bracket(&b, Pos::new(0, 2)).unwrap();
        assert_eq!(a, Pos::new(0, 1));
        assert_eq!(m, Pos::new(0, 0));
    }

    #[test]
    fn matches_across_lines_with_nesting() {
        let b = buf("fn x() {\n    if y {\n    }\n}");
        // the outer '{' is at line 0 col 7
        let (a, m) = matching_bracket(&b, Pos::new(0, 7)).unwrap();
        assert_eq!(a, Pos::new(0, 7));
        assert_eq!(m, Pos::new(3, 0)); // final '}'
    }

    #[test]
    fn no_match_when_unbalanced() {
        let b = buf("foo(bar");
        assert!(matching_bracket(&b, Pos::new(0, 3)).is_none());
    }

    #[test]
    fn none_when_not_on_bracket() {
        let b = buf("hello");
        assert!(matching_bracket(&b, Pos::new(0, 2)).is_none());
    }
}
