pub mod delta;
pub mod piece_table;

pub use delta::{Delta, DeltaKind};
pub use piece_table::PieceTable;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_BUFFER_ID: AtomicU64 = AtomicU64::new(1);

/// Stable, unique identifier for an open buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufferId(u64);

impl BufferId {
    pub fn next() -> Self {
        Self(NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed))
    }
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// Line + column position (both 0-indexed, column in bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
    pub const fn zero() -> Self {
        Self { line: 0, col: 0 }
    }
}

/// A half-open byte-level range within a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub const fn new(start: Pos, end: Pos) -> Self {
        Self { start, end }
    }
    pub const fn empty(pos: Pos) -> Self {
        Self { start: pos, end: pos }
    }
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// What kind of content a buffer holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferKind {
    File(PathBuf),
    Scratch,
    Search,
    References,
    FileTree,
    Terminal,
    /// A raster image file (PNG/JPEG) shown in the pane, not edited as text.
    Image(PathBuf),
}

/// A text buffer backed by an in-house piece table with undo/redo.
pub struct Buffer {
    pub id: BufferId,
    pub kind: BufferKind,
    table: PieceTable,
    undo_stack: Vec<Delta>,
    redo_stack: Vec<Delta>,
    dirty: bool,
    save_marker: usize,
}

impl Buffer {
    pub fn new_scratch() -> Self {
        Self::init(BufferId::next(), BufferKind::Scratch, PieceTable::new(""))
    }

    pub fn from_text(content: &str) -> Self {
        Self::init(BufferId::next(), BufferKind::Scratch, PieceTable::new(content))
    }

    pub fn virtual_buffer(kind: BufferKind, content: &str) -> Self {
        Self::init(BufferId::next(), kind, PieceTable::new(content))
    }

    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(&path)?;
        Ok(Self::init(
            BufferId::next(),
            BufferKind::File(path),
            PieceTable::new(&content),
        ))
    }

    /// An image buffer: the file is rendered, not loaded as text. Holds no
    /// piece-table content; the GUI decodes the path on demand.
    pub fn open_image(path: PathBuf) -> Self {
        Self::init(BufferId::next(), BufferKind::Image(path), PieceTable::new(""))
    }

    fn init(id: BufferId, kind: BufferKind, table: PieceTable) -> Self {
        Self {
            id,
            kind,
            table,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
            save_marker: 0,
        }
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if let BufferKind::File(path) = &self.kind {
            std::fs::write(path, self.table.text())?;
        }
        self.dirty = false;
        self.save_marker = self.undo_stack.len();
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn text(&self) -> String {
        self.table.text()
    }

    /// Replace the entire buffer contents (used for streamed/generated buffers
    /// like the terminal). Clears undo history; does not mark the buffer dirty.
    pub fn set_text(&mut self, content: &str) {
        self.table = PieceTable::new(content);
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn line_count(&self) -> usize {
        self.table.line_count()
    }

    pub fn line(&self, idx: usize) -> Option<String> {
        self.table.line(idx)
    }

    /// Length of a line in bytes (excludes the newline).
    pub fn line_len(&self, line: usize) -> usize {
        self.table.line(line).map(|l| l.len()).unwrap_or(0)
    }

    pub fn pos_to_offset(&self, pos: Pos) -> usize {
        self.table.pos_to_offset(pos.line, pos.col)
    }

    pub fn offset_to_pos(&self, offset: usize) -> Pos {
        let (line, col) = self.table.offset_to_pos(offset);
        Pos { line, col }
    }

    // --- editing ---

    /// Insert `text` at `pos`. Records an undo entry. Returns the delta.
    pub fn insert(&mut self, pos: Pos, text: &str) -> Delta {
        let offset = self.pos_to_offset(pos);
        self.table.insert(offset, text);
        let delta = Delta { kind: DeltaKind::Insert { offset, text: text.to_string() } };
        self.push_undo(delta.clone());
        delta
    }

    /// Delete the half-open span `[start, end)`. Returns the delta.
    pub fn delete_span(&mut self, start: Pos, end: Pos) -> Delta {
        let a = self.pos_to_offset(start);
        let b = self.pos_to_offset(end);
        if a >= b {
            return Delta { kind: DeltaKind::Insert { offset: a, text: String::new() } };
        }
        let deleted = self.table.delete(a, b - a);
        let delta = Delta { kind: DeltaKind::Delete { offset: a, text: deleted } };
        self.push_undo(delta.clone());
        delta
    }

    /// Delete `len` bytes starting at `offset`. Returns the delta.
    pub fn delete_at(&mut self, offset: usize, len: usize) -> Option<Delta> {
        if len == 0 {
            return None;
        }
        let deleted = self.table.delete(offset, len);
        if deleted.is_empty() {
            return None;
        }
        let delta = Delta { kind: DeltaKind::Delete { offset, text: deleted } };
        self.push_undo(delta.clone());
        Some(delta)
    }

    /// Undo the most recent edit. Returns the cursor position after undo, or None if nothing to undo.
    pub fn undo(&mut self) -> Option<Pos> {
        self.undo_with_delta().map(|(pos, _)| pos)
    }

    /// Undo and return both the cursor position and the inverse delta applied.
    pub fn undo_with_delta(&mut self) -> Option<(Pos, Delta)> {
        let delta = self.undo_stack.pop()?;
        let pos = self.invert_and_apply(&delta);
        let applied = delta.inverse();
        self.redo_stack.push(delta);
        self.dirty = self.undo_stack.len() != self.save_marker;
        Some((pos, applied))
    }

    /// Redo the most recently undone edit.
    pub fn redo(&mut self) -> Option<Pos> {
        self.redo_with_delta().map(|(pos, _)| pos)
    }

    /// Redo and return both the cursor position and the delta applied.
    pub fn redo_with_delta(&mut self) -> Option<(Pos, Delta)> {
        let delta = self.redo_stack.pop()?;
        let pos = self.reapply(&delta);
        self.undo_stack.push(delta.clone());
        self.dirty = true;
        Some((pos, delta))
    }

    fn push_undo(&mut self, delta: Delta) {
        self.undo_stack.push(delta);
        self.redo_stack.clear();
        self.dirty = true;
    }

    fn invert_and_apply(&mut self, delta: &Delta) -> Pos {
        match &delta.kind {
            DeltaKind::Insert { offset, text } => {
                self.table.delete(*offset, text.len());
                self.table.offset_to_pos(*offset).into()
            }
            DeltaKind::Delete { offset, text } => {
                self.table.insert(*offset, text);
                let end = offset + text.len();
                self.table.offset_to_pos(end).into()
            }
        }
    }

    fn reapply(&mut self, delta: &Delta) -> Pos {
        match &delta.kind {
            DeltaKind::Insert { offset, text } => {
                self.table.insert(*offset, text);
                let end = offset + text.len();
                self.table.offset_to_pos(end).into()
            }
            DeltaKind::Delete { offset, text } => {
                self.table.delete(*offset, text.len());
                self.table.offset_to_pos(*offset).into()
            }
        }
    }
}

impl From<(usize, usize)> for Pos {
    fn from((line, col): (usize, usize)) -> Self {
        Pos { line, col }
    }
}

/// Central store for all open buffers, keyed by BufferId.
#[derive(Default)]
pub struct BufferStore {
    buffers: HashMap<BufferId, Buffer>,
}

impl BufferStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, buf: Buffer) -> BufferId {
        let id = buf.id;
        self.buffers.insert(id, buf);
        id
    }

    pub fn get(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(&id)
    }

    pub fn get_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers.get_mut(&id)
    }

    pub fn remove(&mut self, id: BufferId) -> Option<Buffer> {
        self.buffers.remove(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffers.keys().copied()
    }
}
