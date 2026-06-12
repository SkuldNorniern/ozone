//! In-house piece table: two-buffer (original + add), vec-of-pieces.
//! All coordinates are byte offsets; callers convert to/from (line, col).

use std::cell::{Ref, RefCell};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Buf {
    Original,
    Add,
}

#[derive(Debug, Clone)]
struct Piece {
    buf: Buf,
    start: usize,
    len: usize,
}

pub struct PieceTable {
    original: Vec<u8>,
    add: Vec<u8>,
    pieces: Vec<Piece>,
    /// Lazily materialized full text, reused across reads within a frame and
    /// invalidated on every edit. Without this, each `line()` / position
    /// conversion rebuilt the whole buffer — O(n) per visible line per frame.
    cache: RefCell<Option<String>>,
}

impl PieceTable {
    pub fn new(content: &str) -> Self {
        let original = content.as_bytes().to_vec();
        let pieces = if original.is_empty() {
            vec![]
        } else {
            vec![Piece {
                buf: Buf::Original,
                start: 0,
                len: original.len(),
            }]
        };
        Self {
            original,
            add: Vec::new(),
            pieces,
            cache: RefCell::new(None),
        }
    }

    /// Borrow the materialized full text, building (and caching) it on demand.
    fn materialized(&self) -> Ref<'_, str> {
        {
            let mut cache = self.cache.borrow_mut();
            if cache.is_none() {
                let mut out = String::with_capacity(self.total_len());
                for p in &self.pieces {
                    out.push_str(self.piece_str(p));
                }
                *cache = Some(out);
            }
        }
        Ref::map(self.cache.borrow(), |c| c.as_deref().unwrap_or(""))
    }

    /// Drop the cached text after a mutation.
    fn invalidate(&mut self) {
        self.cache.get_mut().take();
    }

    pub fn total_len(&self) -> usize {
        self.pieces.iter().map(|p| p.len).sum()
    }

    /// Collect full text. O(n) on a cold cache, O(1) clone when warm.
    pub fn text(&self) -> String {
        self.materialized().to_string()
    }

    pub fn text_eq(&self, other: &str) -> bool {
        &*self.materialized() == other
    }

    pub fn line_count(&self) -> usize {
        let mut n = 1usize;
        for p in &self.pieces {
            n += self.piece_bytes(p).iter().filter(|&&b| b == b'\n').count();
        }
        n
    }

    pub fn line(&self, idx: usize) -> Option<String> {
        let text = self.materialized();
        text.split('\n').nth(idx).map(|s| s.to_string())
    }

    /// Length of one line in bytes, excluding its newline.
    pub fn line_len(&self, idx: usize) -> Option<usize> {
        let text = self.materialized();
        text.split('\n').nth(idx).map(str::len)
    }

    /// Return lines `start..end` in a single pass — O(text_len) instead of
    /// O(end² ) from repeated `nth()` calls.
    pub fn lines_slice(&self, start: usize, end: usize) -> Vec<String> {
        if start >= end {
            return Vec::new();
        }
        let text = self.materialized();
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for (i, line) in text.split('\n').enumerate() {
            if i >= end {
                break;
            }
            if i >= start {
                out.push(line.to_string());
            }
        }
        out
    }

    /// Convert (line, col) to byte offset. Clamps to total length.
    pub fn pos_to_offset(&self, line: usize, col: usize) -> usize {
        let text = self.materialized();
        let total = text.len();

        if line == 0 {
            return col.min(total);
        }

        let mut cur = 0usize;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                cur += 1;
                if cur == line {
                    return (i + 1 + col).min(total);
                }
            }
        }
        total
    }

    /// Convert byte offset to (line, col). Snaps to a char boundary so callers
    /// passing an offset that lands inside a multi-byte char never panic.
    pub fn offset_to_pos(&self, offset: usize) -> (usize, usize) {
        let text = self.materialized();
        let mut offset = offset.min(text.len());
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let prefix = &text[..offset];
        let line = prefix.bytes().filter(|&b| b == b'\n').count();
        let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
        (line, offset - line_start)
    }

    pub fn insert(&mut self, offset: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let add_start = self.add.len();
        self.add.extend_from_slice(text.as_bytes());
        let new_piece = Piece {
            buf: Buf::Add,
            start: add_start,
            len: text.len(),
        };

        let (pi, inner) = self.find_piece(offset);

        if pi == self.pieces.len() {
            self.pieces.push(new_piece);
        } else if inner == 0 {
            self.pieces.insert(pi, new_piece);
        } else {
            let old = self.pieces[pi].clone();
            let left = Piece {
                buf: old.buf,
                start: old.start,
                len: inner,
            };
            let right = Piece {
                buf: old.buf,
                start: old.start + inner,
                len: old.len - inner,
            };
            self.pieces.splice(pi..=pi, [left, new_piece, right]);
        }
        self.invalidate();
    }

    /// Delete `len` bytes starting at `offset`. Returns deleted text.
    pub fn delete(&mut self, offset: usize, len: usize) -> String {
        if len == 0 {
            return String::new();
        }
        let end_offset = (offset + len).min(self.total_len());
        let actual_len = end_offset - offset;
        if actual_len == 0 {
            return String::new();
        }

        // Collect deleted bytes for undo
        let deleted = {
            let text = self.materialized();
            text[offset..end_offset].to_string()
        };

        let (sp, si) = self.find_piece(offset);
        let (ep, ei) = self.find_piece(end_offset);

        let mut replacement = Vec::new();

        // Left fragment of start piece
        if si > 0 {
            let p = &self.pieces[sp];
            replacement.push(Piece {
                buf: p.buf,
                start: p.start,
                len: si,
            });
        }

        // Right fragment of end piece (if it exists and has a tail)
        if ep < self.pieces.len() && ei < self.pieces[ep].len {
            let p = &self.pieces[ep];
            replacement.push(Piece {
                buf: p.buf,
                start: p.start + ei,
                len: p.len - ei,
            });
        }

        let splice_end = if ep < self.pieces.len() {
            ep + 1
        } else {
            self.pieces.len()
        };
        self.pieces.splice(sp..splice_end, replacement);

        self.invalidate();
        deleted
    }

    // --- helpers ---

    fn piece_bytes<'a>(&'a self, p: &Piece) -> &'a [u8] {
        let buf = match p.buf {
            Buf::Original => &self.original,
            Buf::Add => &self.add,
        };
        &buf[p.start..p.start + p.len]
    }

    fn piece_str<'a>(&'a self, p: &Piece) -> &'a str {
        // SAFETY: all inserted text is valid UTF-8 (we only accept &str).
        unsafe { std::str::from_utf8_unchecked(self.piece_bytes(p)) }
    }

    /// Find which piece contains `offset` and the inner byte offset within it.
    /// Uses strict `<` so boundary offsets map to the START of the next piece.
    fn find_piece(&self, offset: usize) -> (usize, usize) {
        let mut remaining = offset;
        for (i, p) in self.pieces.iter().enumerate() {
            if remaining < p.len {
                return (i, remaining);
            }
            remaining -= p.len;
        }
        (self.pieces.len(), remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty() {
        let t = PieceTable::new("");
        assert_eq!(t.text(), "");
        assert_eq!(t.line_count(), 1);
    }

    #[test]
    fn insert_at_end() {
        let mut t = PieceTable::new("hello");
        t.insert(5, " world");
        assert_eq!(t.text(), "hello world");
    }

    #[test]
    fn insert_at_start() {
        let mut t = PieceTable::new("world");
        t.insert(0, "hello ");
        assert_eq!(t.text(), "hello world");
    }

    #[test]
    fn insert_middle() {
        let mut t = PieceTable::new("helloworld");
        t.insert(5, " ");
        assert_eq!(t.text(), "hello world");
    }

    #[test]
    fn delete_within_piece() {
        let mut t = PieceTable::new("hello world");
        let d = t.delete(5, 6);
        assert_eq!(d, " world");
        assert_eq!(t.text(), "hello");
    }

    #[test]
    fn delete_across_pieces() {
        let mut t = PieceTable::new("hello");
        t.insert(5, " world");
        // text is now "hello world"
        let d = t.delete(3, 5); // delete "lo wo"
        assert_eq!(d, "lo wo");
        assert_eq!(t.text(), "helrld");
    }

    #[test]
    fn line_count_multiline() {
        let t = PieceTable::new("a\nb\nc");
        assert_eq!(t.line_count(), 3);
    }

    #[test]
    fn line_len_does_not_require_an_owned_line() {
        let t = PieceTable::new("alpha\n한글\n");
        assert_eq!(t.line_len(0), Some(5));
        assert_eq!(t.line_len(1), Some(6));
        assert_eq!(t.line_len(2), Some(0));
        assert_eq!(t.line_len(3), None);
    }

    #[test]
    fn pos_to_offset_line0() {
        let t = PieceTable::new("hello\nworld");
        assert_eq!(t.pos_to_offset(0, 3), 3);
    }

    #[test]
    fn pos_to_offset_line1() {
        let t = PieceTable::new("hello\nworld");
        assert_eq!(t.pos_to_offset(1, 2), 8); // 'h'=6, 'e'=7, 'r'=8
    }

    #[test]
    fn offset_to_pos() {
        let t = PieceTable::new("hello\nworld");
        assert_eq!(t.offset_to_pos(0), (0, 0));
        // offset 5 is the '\n' — cursor sits at end of line 0, col 5
        assert_eq!(t.offset_to_pos(5), (0, 5));
        // offset 6 is the 'w' — start of line 1
        assert_eq!(t.offset_to_pos(6), (1, 0));
        assert_eq!(t.offset_to_pos(8), (1, 2));
    }

    #[test]
    fn undo_insert() {
        let mut t = PieceTable::new("hello");
        t.insert(5, " world");
        assert_eq!(t.text(), "hello world");
        t.delete(5, 6);
        assert_eq!(t.text(), "hello");
    }

    #[test]
    fn cache_invalidates_on_edit() {
        let mut t = PieceTable::new("ab\ncd");
        // warm the cache
        assert_eq!(t.line(1).as_deref(), Some("cd"));
        t.insert(0, "X");
        // a stale cache would still report the old line contents
        assert_eq!(t.text(), "Xab\ncd");
        assert_eq!(t.line(0).as_deref(), Some("Xab"));
        t.delete(0, 1);
        assert_eq!(t.line(0).as_deref(), Some("ab"));
    }

    #[test]
    fn offset_to_pos_snaps_to_char_boundary() {
        // "é" is two bytes (0xC3 0xA9); an offset of 1 lands mid-char.
        let t = PieceTable::new("é\nx");
        // must not panic and must clamp back to the start of the char
        assert_eq!(t.offset_to_pos(1), (0, 0));
        assert_eq!(t.offset_to_pos(2), (0, 2)); // end of line 0
        assert_eq!(t.offset_to_pos(3), (1, 0)); // start of line 1
    }

    #[test]
    fn repeated_line_reads_are_consistent() {
        let t = PieceTable::new("one\ntwo\nthree");
        for _ in 0..3 {
            assert_eq!(t.line(0).as_deref(), Some("one"));
            assert_eq!(t.line(2).as_deref(), Some("three"));
            assert_eq!(t.line(3), None);
        }
    }
}
