//! Decorations (extmarks): edit-tracking, namespaced annotations over buffer
//! byte ranges.
//!
//! One model behind everything that paints *over* the text without being text:
//! search/match highlights, matching-bracket boxes, LSP diagnostics, git signs,
//! inline hints. The editor owns the model (positions + edit tracking); the
//! frontend decides how each [`HlRole`] looks (theme-mapped), so this stays
//! windowing- and palette-free. Plugins/LSP add and clear by [`NamespaceId`].
//!
//! Positions are byte offsets into the whole buffer and follow edits: see
//! [`DecorationStore::apply_delta`].

use std::collections::HashMap;

use ozone_buffer::{BufferId, Delta, DeltaKind};

/// A namespace groups decorations for one feature/plugin so they can be cleared
/// atomically (e.g. "clear all diagnostics, then re-add"). Issued by
/// [`DecorationStore::namespace`].
pub type NamespaceId = u32;

/// Opaque handle to one decoration, for targeted removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecorationId(u64);

/// Which side of an insertion a decoration endpoint remains attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gravity {
    Left,
    Right,
}

/// Semantic highlight role. The frontend theme maps a role to a colour, so the
/// editor never names a palette entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlRole {
    Search,
    SearchCurrent,
    Bracket,
    Selection,
    Error,
    Warn,
    Info,
    Hint,
}

/// Where virtual text sits relative to its anchor offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualPos {
    /// At end of the anchor's line.
    Eol,
    /// Inline at the anchor offset (shifts following text visually).
    Inline,
}

/// What a decoration draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecorationKind {
    /// Background highlight over `[start, end)`.
    Highlight(HlRole),
    /// Underline over `[start, end)` (e.g. diagnostics).
    Underline(HlRole),
    /// A short sign in the gutter on the start line.
    GutterSign(String),
    /// Virtual (non-buffer) text anchored at `start`.
    VirtualText {
        text: String,
        pos: VirtualPos,
        role: HlRole,
    },
}

/// One decoration: a byte range + what it draws, tagged with its namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoration {
    pub id: DecorationId,
    pub namespace: NamespaceId,
    /// Byte-offset range, edit-tracked. Invariant: `end >= start`.
    pub start: usize,
    pub end: usize,
    pub start_gravity: Gravity,
    pub end_gravity: Gravity,
    pub kind: DecorationKind,
}

/// Per-buffer decoration storage with edit tracking and namespaced clear.
#[derive(Debug, Default)]
pub struct DecorationStore {
    by_buffer: HashMap<BufferId, Vec<Decoration>>,
    next_id: u64,
    next_ns: NamespaceId,
}

impl DecorationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve a fresh namespace (one per feature/plugin).
    pub fn namespace(&mut self) -> NamespaceId {
        self.next_ns = self
            .next_ns
            .checked_add(1)
            .expect("decoration namespace space exhausted");
        self.next_ns
    }

    /// Add a decoration over `[start, end)`. By default, insertion at the start
    /// moves the range right and insertion at the end stays outside the range.
    pub fn add(
        &mut self,
        buffer: BufferId,
        namespace: NamespaceId,
        start: usize,
        end: usize,
        kind: DecorationKind,
    ) -> DecorationId {
        self.add_with_gravity(
            buffer,
            namespace,
            start,
            end,
            Gravity::Right,
            Gravity::Left,
            kind,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_with_gravity(
        &mut self,
        buffer: BufferId,
        namespace: NamespaceId,
        start: usize,
        end: usize,
        start_gravity: Gravity,
        end_gravity: Gravity,
        kind: DecorationKind,
    ) -> DecorationId {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("decoration id space exhausted");
        let id = DecorationId(self.next_id);
        let (start, end) = if start <= end { (start, end) } else { (end, start) };
        self.by_buffer.entry(buffer).or_default().push(Decoration {
            id,
            namespace,
            start,
            end,
            start_gravity,
            end_gravity,
            kind,
        });
        id
    }

    /// Remove every decoration in `namespace` across all buffers. Returns count.
    pub fn clear_namespace(&mut self, namespace: NamespaceId) -> usize {
        let mut removed = 0;
        for v in self.by_buffer.values_mut() {
            let before = v.len();
            v.retain(|d| d.namespace != namespace);
            removed += before - v.len();
        }
        removed
    }

    /// Remove every decoration in `namespace` from one buffer.
    pub fn clear_namespace_in(&mut self, buffer: BufferId, namespace: NamespaceId) -> usize {
        let Some(v) = self.by_buffer.get_mut(&buffer) else {
            return 0;
        };
        let before = v.len();
        v.retain(|d| d.namespace != namespace);
        before - v.len()
    }

    /// Remove one decoration by id. Returns whether it existed.
    pub fn remove(&mut self, id: DecorationId) -> bool {
        for v in self.by_buffer.values_mut() {
            if let Some(i) = v.iter().position(|d| d.id == id) {
                v.remove(i);
                return true;
            }
        }
        false
    }

    /// Decorations on `buffer` overlapping `[start, end)`, in document order
    /// (by start, then end). Zero-width decorations count as a point.
    pub fn in_range(&self, buffer: BufferId, start: usize, end: usize) -> Vec<&Decoration> {
        let Some(v) = self.by_buffer.get(&buffer) else {
            return Vec::new();
        };
        let mut out: Vec<&Decoration> = v
            .iter()
            .filter(|d| {
                if d.start == d.end {
                    d.start >= start && d.start < end
                } else {
                    d.start < end && d.end > start
                }
            })
            .collect();
        out.sort_by_key(|d| (d.start, d.end, d.id.0));
        out
    }

    /// All decorations on `buffer`, unordered.
    pub fn all(&self, buffer: BufferId) -> &[Decoration] {
        self.by_buffer
            .get(&buffer)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Shift decoration offsets on `buffer` to follow an edit. Endpoint gravity
    /// decides which side of a boundary insertion an endpoint remains on. A
    /// deletion of `[o, o+n)` pulls later positions left and clamps positions
    /// inside the hole to `o`.
    pub fn apply_delta(&mut self, buffer: BufferId, delta: &Delta) {
        let Some(v) = self.by_buffer.get_mut(&buffer) else {
            return;
        };
        match &delta.kind {
            DeltaKind::Insert { offset, text } => {
                let (o, len) = (*offset, text.len());
                for d in v.iter_mut() {
                    if d.start == d.end {
                        if d.start > o
                            || (d.start == o && d.start_gravity == Gravity::Right)
                        {
                            d.start += len;
                            d.end += len;
                        }
                        continue;
                    }
                    if d.start > o || (d.start == o && d.start_gravity == Gravity::Right) {
                        d.start += len;
                    }
                    if d.end > o || (d.end == o && d.end_gravity == Gravity::Right) {
                        d.end += len;
                    }
                }
            }
            DeltaKind::Delete { offset, text } => {
                let (o, len) = (*offset, text.len());
                let end = o + len;
                let shift = |p: usize| {
                    if p <= o {
                        p
                    } else if p >= end {
                        p - len
                    } else {
                        o
                    }
                };
                for d in v.iter_mut() {
                    d.start = shift(d.start);
                    d.end = shift(d.end);
                }
            }
        }
    }

    /// Drop all decorations for a closed buffer.
    pub fn forget_buffer(&mut self, buffer: BufferId) {
        self.by_buffer.remove(&buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozone_buffer::BufferId;

    fn ins(offset: usize, text: &str) -> Delta {
        Delta {
            kind: DeltaKind::Insert {
                offset,
                text: text.to_string(),
            },
        }
    }
    fn del(offset: usize, text: &str) -> Delta {
        Delta {
            kind: DeltaKind::Delete {
                offset,
                text: text.to_string(),
            },
        }
    }

    fn hl(store: &mut DecorationStore, buf: BufferId, ns: NamespaceId, s: usize, e: usize) -> DecorationId {
        store.add(buf, ns, s, e, DecorationKind::Highlight(HlRole::Search))
    }

    #[test]
    fn add_and_query_overlap() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        hl(&mut s, b, ns, 2, 5);
        hl(&mut s, b, ns, 10, 12);
        // [0,3) overlaps the first only; [12,20) overlaps neither (end exclusive).
        assert_eq!(s.in_range(b, 0, 3).len(), 1);
        assert_eq!(s.in_range(b, 12, 20).len(), 0);
        assert_eq!(s.in_range(b, 0, 50).len(), 2);
    }

    #[test]
    fn insert_before_shifts_whole_range() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        hl(&mut s, b, ns, 4, 8);
        s.apply_delta(b, &ins(0, "xx")); // 2 bytes before
        let d = &s.all(b)[0];
        assert_eq!((d.start, d.end), (6, 10));
    }

    #[test]
    fn insert_inside_grows_range() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        hl(&mut s, b, ns, 4, 8);
        s.apply_delta(b, &ins(6, "zzz")); // inside [4,8): start stays, end grows
        let d = &s.all(b)[0];
        assert_eq!((d.start, d.end), (4, 11));
    }

    #[test]
    fn default_gravity_excludes_boundary_insertions() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        hl(&mut s, b, ns, 4, 8);
        s.apply_delta(b, &ins(4, "a"));
        assert_eq!((s.all(b)[0].start, s.all(b)[0].end), (5, 9));
        s.apply_delta(b, &ins(9, "b"));
        assert_eq!((s.all(b)[0].start, s.all(b)[0].end), (5, 9));
    }

    #[test]
    fn point_mark_uses_start_gravity() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        s.add(
            b,
            ns,
            4,
            4,
            DecorationKind::VirtualText {
                text: "hint".to_string(),
                pos: VirtualPos::Inline,
                role: HlRole::Hint,
            },
        );
        s.apply_delta(b, &ins(4, "x"));
        assert_eq!((s.all(b)[0].start, s.all(b)[0].end), (5, 5));
    }

    #[test]
    fn delete_after_pulls_left_and_inside_clamps() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        hl(&mut s, b, ns, 10, 14); // wholly after the deletion
        hl(&mut s, b, ns, 3, 7); //  straddles into the deletion [4,8)
        s.apply_delta(b, &del(4, "abcd")); // delete 4 bytes at offset 4 → [4,8)
        // first deco shifts left by 4
        let after: Vec<(usize, usize)> = s.all(b).iter().map(|d| (d.start, d.end)).collect();
        assert!(after.contains(&(6, 10))); // 10..14 → 6..10
        assert!(after.contains(&(3, 4))); // 3 stays, 7 clamps to 4
    }

    #[test]
    fn namespace_clear_is_atomic() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let a = s.namespace();
        let c = s.namespace();
        hl(&mut s, b, a, 0, 1);
        hl(&mut s, b, a, 2, 3);
        hl(&mut s, b, c, 4, 5);
        assert_eq!(s.clear_namespace(a), 2);
        assert_eq!(s.all(b).len(), 1);
        assert_eq!(s.all(b)[0].namespace, c);
    }

    #[test]
    fn namespace_can_be_cleared_per_buffer() {
        let mut s = DecorationStore::new();
        let a = BufferId::next();
        let b = BufferId::next();
        let ns = s.namespace();
        hl(&mut s, a, ns, 0, 1);
        hl(&mut s, b, ns, 0, 1);
        assert_eq!(s.clear_namespace_in(a, ns), 1);
        assert!(s.all(a).is_empty());
        assert_eq!(s.all(b).len(), 1);
    }

    #[test]
    fn remove_by_id() {
        let mut s = DecorationStore::new();
        let b = BufferId::next();
        let ns = s.namespace();
        let id = hl(&mut s, b, ns, 0, 1);
        assert!(s.remove(id));
        assert!(!s.remove(id));
        assert!(s.all(b).is_empty());
    }
}
