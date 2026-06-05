//! Literal in-buffer search (no regex, per the dependency policy).
//!
//! Returns byte offsets of every non-overlapping match in the original text, so
//! the offsets stay valid for `offset_to_pos`. Case-insensitive matching folds
//! ASCII only, which keeps positions byte-accurate for multi-byte text.

/// All non-overlapping match start offsets of `query` in `text`.
pub fn find_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<usize> {
    if query.is_empty() || query.len() > text.len() {
        return Vec::new();
    }
    let tb = text.as_bytes();
    let qb = query.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + qb.len() <= tb.len() {
        let window = &tb[i..i + qb.len()];
        let hit = if case_sensitive {
            window == qb
        } else {
            window.eq_ignore_ascii_case(qb)
        };
        if hit {
            out.push(i);
            i += qb.len(); // non-overlapping
        } else {
            i += 1;
        }
    }
    out
}

/// Index of the first match at or after `from`, else the first match (wrap),
/// else `None`.
pub fn first_match_from(matches: &[usize], from: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    match matches.iter().position(|&m| m >= from) {
        Some(i) => Some(i),
        None => Some(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_all_non_overlapping() {
        assert_eq!(find_matches("abcabc", "abc", true), vec![0, 3]);
        assert_eq!(find_matches("aaaa", "aa", true), vec![0, 2]); // non-overlapping
        assert_eq!(find_matches("xyz", "q", true), Vec::<usize>::new());
    }

    #[test]
    fn case_insensitive_default() {
        assert_eq!(find_matches("Foo foo FOO", "foo", false), vec![0, 4, 8]);
        assert_eq!(find_matches("Foo foo FOO", "foo", true), vec![4]);
    }

    #[test]
    fn empty_query_no_matches() {
        assert!(find_matches("anything", "", false).is_empty());
    }

    #[test]
    fn offsets_valid_with_multibyte() {
        // "héllo héllo" — match "llo" by byte offset; é is 2 bytes.
        let text = "héllo héllo";
        let m = find_matches(text, "llo", false);
        // each match offset must land on a char boundary
        for &off in &m {
            assert!(text.is_char_boundary(off));
            assert_eq!(&text[off..off + 3], "llo");
        }
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn first_match_from_wraps() {
        let m = vec![2, 10, 20];
        assert_eq!(first_match_from(&m, 0), Some(0));
        assert_eq!(first_match_from(&m, 5), Some(1));
        assert_eq!(first_match_from(&m, 25), Some(0)); // wrap
        assert_eq!(first_match_from(&[], 0), None);
    }
}
