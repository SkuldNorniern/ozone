//! Indentation-based code folding (Layer-0, language-agnostic).
//!
//! A line is a *fold header* if the lines below it are more indented; its fold
//! region runs to the last such deeper line (blank lines inside don't break it).
//! No structural parser — deterministic and works for any indented text. Fold
//! state (which headers are collapsed) is view-local; see [`crate::view::View`].

use std::collections::HashSet;

use ozone_buffer::Buffer;

/// Leading-whitespace width (tabs and spaces each count as one column).
fn indent_width(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The fold region a `header` line opens, as inclusive line range
/// `(header, last)`. `None` when nothing below it is more indented (so there's
/// nothing to fold). Blank lines within the deeper block are absorbed but do not
/// extend the region past the last deeper non-blank line.
pub fn fold_region(buf: &Buffer, header: usize) -> Option<(usize, usize)> {
    let n = buf.line_count();
    if header >= n {
        return None;
    }
    let h_indent = indent_width(&buf.line(header)?);
    let mut last = header;
    let mut i = header + 1;
    while i < n {
        let line = buf.line(i).unwrap_or_default();
        if is_blank(&line) {
            i += 1;
            continue;
        }
        if indent_width(&line) > h_indent {
            last = i;
            i += 1;
        } else {
            break;
        }
    }
    (last > header).then_some((header, last))
}

/// Whether `header` can be folded (opens a non-empty region).
pub fn is_foldable(buf: &Buffer, header: usize) -> bool {
    fold_region(buf, header).is_some()
}

/// Whether the line looks like it opens a block (ends with `{`, `(`, `[`, or `:`).
/// Used to filter out continuation lines, split assignments, etc. from showing
/// as fold headers — only real block-openers get the gutter indicator.
fn is_block_opener(line: &str) -> bool {
    matches!(
        line.trim_end().chars().last(),
        Some('{' | '(' | '[' | ':')
    )
}

/// Whether `header` should show a fold indicator: must both open a region AND
/// end with a block-opener character so continuation lines are excluded.
pub fn is_visual_fold_header(buf: &Buffer, header: usize) -> bool {
    let Some(line) = buf.line(header) else { return false };
    is_block_opener(&line) && fold_region(buf, header).is_some()
}

/// Whether `line` is hidden by some collapsed fold in `folds` — i.e. it lies
/// strictly inside a folded header's region. The header line itself stays
/// visible (it shows the fold marker).
pub fn is_hidden(buf: &Buffer, folds: &HashSet<usize>, line: usize) -> bool {
    folds.iter().any(|&h| {
        h < line && fold_region(buf, h).is_some_and(|(start, end)| line > start && line <= end)
    })
}

/// The nearest visual fold header at or above `line` whose region contains `line`
/// (or `line` itself if it is a header). Used by "toggle fold at cursor".
pub fn header_for(buf: &Buffer, line: usize) -> Option<usize> {
    if is_visual_fold_header(buf, line) {
        return Some(line);
    }
    (0..line)
        .rev()
        .find(|&h| is_visual_fold_header(buf, h) && fold_region(buf, h).is_some_and(|(start, end)| line > start && line <= end))
}

/// Every visual fold header line in the buffer (for "fold all").
pub fn all_headers(buf: &Buffer) -> Vec<usize> {
    (0..buf.line_count())
        .filter(|&l| is_visual_fold_header(buf, l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::from_text(s)
    }

    #[test]
    fn region_spans_more_indented_lines() {
        let b = buf("fn x() {\n    a;\n    b;\n}\nafter");
        // header line 0 (`fn x() {`) folds lines 1..=2 (the indented body).
        assert_eq!(fold_region(&b, 0), Some((0, 2)));
        // the `}` (line 3) is at the header indent → not part of the region.
        assert!(!is_foldable(&b, 3));
    }

    #[test]
    fn blank_lines_inside_do_not_break_region() {
        let b = buf("root\n  a\n\n  b\ntail");
        assert_eq!(fold_region(&b, 0), Some((0, 3)));
    }

    #[test]
    fn nested_indentation() {
        let b = buf("a\n  b\n    c\n  d\ne");
        assert_eq!(fold_region(&b, 0), Some((0, 3)));
        assert_eq!(fold_region(&b, 1), Some((1, 2)));
        assert!(!is_foldable(&b, 4));
    }

    #[test]
    fn hidden_lines_inside_folded_header() {
        let b = buf("a\n  b\n  c\nd");
        let folds = HashSet::from([0usize]);
        assert!(!is_hidden(&b, &folds, 0)); // header visible
        assert!(is_hidden(&b, &folds, 1));
        assert!(is_hidden(&b, &folds, 2));
        assert!(!is_hidden(&b, &folds, 3));
    }

    #[test]
    fn header_for_finds_enclosing_fold() {
        let b = buf("a\n  b\n    c\n  d\ne");
        assert_eq!(header_for(&b, 0), Some(0)); // on a foldable header
        assert_eq!(header_for(&b, 2), Some(1)); // line 2 not foldable → nearest enclosing (1)
        assert_eq!(header_for(&b, 3), Some(0)); // inside line 0's region, not 1's
        assert_eq!(header_for(&b, 4), None); // tail, no fold
    }

    #[test]
    fn all_headers_lists_foldables() {
        let b = buf("a\n  b\nc\n  d\n    e");
        assert_eq!(all_headers(&b), vec![0, 2, 3]);
    }
}
