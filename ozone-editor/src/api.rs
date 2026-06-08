//! A small, stable *primitive* surface for embedding Ozone and (later) plugins.
//!
//! Commands ([`crate::CommandRegistry`]) are the coarse, named, user-facing
//! units — the "everything is a command" layer. But extensions also need the
//! *building blocks* those commands are made of: read the cursor, insert text,
//! delete a range, undo, run a search — without reaching into buffer/view/piece
//! table internals.
//!
//! [`EditorApi`] is that layer: a cursor-aware wrapper over a `&mut Workspace`
//! that delegates to the buffer, view, and search modules and emits the right
//! [`EditorEvent`]s. It is deliberately thin and dependency-free so it can back
//! a future plugin host ABI unchanged.
//!
//! ```
//! use ozone_editor::{Workspace, api::EditorApi};
//! let mut ws = Workspace::new();
//! let mut api = EditorApi::new(&mut ws);
//! api.insert("hello");
//! assert_eq!(api.line(0).as_deref(), Some("hello"));
//! ```

use std::path::PathBuf;

use ozone_buffer::{BufferId, Pos, Span};

use crate::decoration::{Decoration, DecorationId, DecorationKind, Gravity, NamespaceId};
use crate::events::EditorEvent;
use crate::options::OptionValue;
use crate::pane::{FocusDirection, SplitAxis};
use crate::search;
use crate::ui::{NotifyLevel, UiIntent};
use crate::view::ViewId;
use crate::workspace::Workspace;

/// Cursor-aware primitive operations on the active view/buffer.
///
/// Borrows the workspace for the duration of a batch of operations; construct
/// one per logical action.
pub struct EditorApi<'a> {
    ws: &'a mut Workspace,
}

impl<'a> EditorApi<'a> {
    pub fn new(ws: &'a mut Workspace) -> Self {
        Self { ws }
    }

    /// Escape hatch to the underlying workspace for operations not yet wrapped.
    pub fn workspace(&self) -> &Workspace {
        self.ws
    }
    pub fn workspace_mut(&mut self) -> &mut Workspace {
        self.ws
    }

    // --- identity ---

    pub fn active_view(&self) -> Option<ViewId> {
        self.ws.active_view_id
    }
    pub fn active_buffer(&self) -> Option<BufferId> {
        self.ws.active_view().map(|v| v.buffer_id)
    }

    // --- queries ---

    pub fn cursor(&self) -> Option<Pos> {
        self.ws.active_view().map(|v| v.cursor)
    }
    pub fn selection(&self) -> Option<Span> {
        self.ws.active_view().and_then(|v| v.selection)
    }
    pub fn line(&self, line: usize) -> Option<String> {
        self.ws.active_buffer().and_then(|b| b.line(line))
    }
    pub fn line_count(&self) -> usize {
        self.ws.active_buffer().map(|b| b.line_count()).unwrap_or(0)
    }
    pub fn text(&self) -> Option<String> {
        self.ws.active_buffer().map(|b| b.text())
    }

    // --- cursor / selection ---

    /// Move the cursor (clamped by callers if needed) and emit `CursorMoved`.
    pub fn set_cursor(&mut self, pos: Pos) {
        let id = match self.ws.active_view_mut() {
            Some(view) => {
                view.cursor = pos;
                view.col_memory = pos.col;
                view.id
            }
            None => return,
        };
        self.ws.emit(EditorEvent::CursorMoved { view_id: id, pos });
    }

    pub fn set_selection(&mut self, selection: Option<Span>) {
        if let Some(view) = self.ws.active_view_mut() {
            view.selection = selection;
        }
    }

    // --- edits (cursor-aware) ---

    /// Insert `text` at the cursor, advancing it by the inserted byte length.
    /// Returns false if there is no active buffer.
    pub fn insert(&mut self, text: &str) -> bool {
        let Some(view) = self.ws.active_view() else {
            return false;
        };
        let (cursor, bid) = (view.cursor, view.buffer_id);
        let Some(buf) = self.ws.buffers.get_mut(&bid) else {
            return false;
        };
        let delta = buf.insert(cursor, text);
        // Columns are byte offsets (see `Pos`), so advance by byte length.
        let bytes = text.len();
        if let Some(view) = self.ws.active_view_mut() {
            view.cursor.col += bytes;
            view.col_memory = view.cursor.col;
        }
        self.ws.emit(EditorEvent::BufferChanged { id: bid, delta });
        true
    }

    /// Delete the half-open range `[start, end)` from the active buffer.
    pub fn delete(&mut self, start: Pos, end: Pos) -> bool {
        let Some(bid) = self.active_buffer() else {
            return false;
        };
        let Some(buf) = self.ws.buffers.get_mut(&bid) else {
            return false;
        };
        let delta = buf.delete_span(start, end);
        self.ws.emit(EditorEvent::BufferChanged { id: bid, delta });
        true
    }

    /// Undo the last edit; moves the cursor to the change site. Returns whether
    /// anything was undone.
    pub fn undo(&mut self) -> bool {
        let Some(bid) = self.active_buffer() else {
            return false;
        };
        let result = self
            .ws
            .buffers
            .get_mut(&bid)
            .and_then(|b| b.undo_with_delta());
        match result {
            Some((pos, delta)) => {
                self.ws.emit(EditorEvent::BufferChanged { id: bid, delta });
                self.set_cursor(pos);
                true
            }
            None => false,
        }
    }

    /// Redo the last undone edit. Returns whether anything was redone.
    pub fn redo(&mut self) -> bool {
        let Some(bid) = self.active_buffer() else {
            return false;
        };
        let result = self
            .ws
            .buffers
            .get_mut(&bid)
            .and_then(|b| b.redo_with_delta());
        match result {
            Some((pos, delta)) => {
                self.ws.emit(EditorEvent::BufferChanged { id: bid, delta });
                self.set_cursor(pos);
                true
            }
            None => false,
        }
    }

    // --- search ---

    /// Byte offsets of every literal match of `query` in the active buffer.
    pub fn find(&self, query: &str, case_sensitive: bool) -> Vec<usize> {
        let text = self.text().unwrap_or_default();
        search::find_matches(&text, query, case_sensitive)
    }

    // --- buffers ---

    /// Ids of all open buffers (order unspecified).
    pub fn buffer_ids(&self) -> Vec<BufferId> {
        self.ws.buffers.keys().copied().collect()
    }
    /// Open a file (or focus it as a buffer); returns success.
    pub fn open_file(&mut self, path: PathBuf) -> bool {
        self.ws.open_file(path).is_ok()
    }
    /// Point the active pane at an already-open buffer.
    pub fn switch_buffer(&mut self, id: BufferId) -> bool {
        self.ws.switch_active_buffer(id)
    }
    /// Cycle the active pane to the next/previous editable buffer.
    pub fn cycle_buffer(&mut self, forward: bool) -> bool {
        self.ws.cycle_buffer(forward)
    }

    // --- panes ---

    /// Split the active pane along `axis`; returns the new view.
    pub fn split(&mut self, axis: SplitAxis) -> Option<ViewId> {
        self.ws.split_active_pane(axis)
    }
    /// Move focus to the neighbouring pane in a direction.
    pub fn focus_pane(&mut self, direction: FocusDirection) -> Option<ViewId> {
        self.ws.focus_pane_in_direction(direction)
    }
    pub fn focus_next_pane(&mut self) -> Option<ViewId> {
        self.ws.focus_next_pane()
    }
    pub fn focus_previous_pane(&mut self) -> Option<ViewId> {
        self.ws.focus_previous_pane()
    }
    /// Close the active pane (no-op if it is the last one). Returns success.
    pub fn close_pane(&mut self) -> bool {
        match self.ws.active_view_id {
            Some(v) => self.ws.close_view(v),
            None => false,
        }
    }

    // --- navigation ---

    /// Record the current location as a jump origin (for `jump_back`).
    pub fn push_jump(&mut self) {
        self.ws.push_jump();
    }
    pub fn jump_back(&mut self) -> bool {
        self.ws.jump_back()
    }
    pub fn jump_forward(&mut self) -> bool {
        self.ws.jump_forward()
    }
    /// Scroll so the cursor is visible in the active view.
    pub fn scroll_to_cursor(&mut self) {
        if let Some(view) = self.ws.active_view_mut() {
            let page = view.page_height.max(1);
            view.scroll_to_cursor(page);
        }
    }

    // --- buffer-local options ---

    /// Override a setting for the active buffer (`tab_width`, `soft_tabs`,
    /// `word_wrap`, `line_numbers`). Mirrors Neovim `vim.bo`.
    pub fn set_local(&mut self, key: &str, value: OptionValue) {
        if let Some(id) = self.active_buffer() {
            self.ws.set_local(id, key, value);
        }
    }

    // --- decorations ---

    /// Reserve a namespace for one feature or extension.
    pub fn decoration_namespace(&mut self) -> NamespaceId {
        self.ws.decorations_mut().namespace()
    }

    /// Add a decoration to the active buffer using default endpoint gravity.
    pub fn add_decoration(
        &mut self,
        namespace: NamespaceId,
        start: usize,
        end: usize,
        kind: DecorationKind,
    ) -> Option<DecorationId> {
        let buffer = self.active_buffer()?;
        Some(
            self.ws
                .decorations_mut()
                .add(buffer, namespace, start, end, kind),
        )
    }

    /// Add a decoration to the active buffer that is visible only in the active
    /// view.
    pub fn add_view_decoration(
        &mut self,
        namespace: NamespaceId,
        start: usize,
        end: usize,
        kind: DecorationKind,
    ) -> Option<DecorationId> {
        let view = self.ws.active_view()?;
        let (view_id, buffer) = (view.id, view.buffer_id);
        Some(
            self.ws
                .decorations_mut()
                .add_for_view(buffer, view_id, namespace, start, end, kind),
        )
    }

    /// Add a decoration to an explicit buffer with configurable endpoint gravity.
    #[allow(clippy::too_many_arguments)]
    pub fn add_decoration_in(
        &mut self,
        buffer: BufferId,
        namespace: NamespaceId,
        start: usize,
        end: usize,
        start_gravity: Gravity,
        end_gravity: Gravity,
        kind: DecorationKind,
    ) -> Option<DecorationId> {
        self.ws.buffers.contains_key(&buffer).then(|| {
            self.ws.decorations_mut().add_with_gravity(
                buffer,
                namespace,
                start,
                end,
                start_gravity,
                end_gravity,
                kind,
            )
        })
    }

    /// Clear a namespace across every buffer.
    pub fn clear_decoration_namespace(&mut self, namespace: NamespaceId) -> usize {
        self.ws.decorations_mut().clear_namespace(namespace)
    }

    /// Clear a namespace in one buffer.
    pub fn clear_decoration_namespace_in(
        &mut self,
        buffer: BufferId,
        namespace: NamespaceId,
    ) -> usize {
        self.ws
            .decorations_mut()
            .clear_namespace_in(buffer, namespace)
    }

    /// Copy decorations overlapping `[start, end)` from an explicit buffer.
    pub fn decorations_in(&self, buffer: BufferId, start: usize, end: usize) -> Vec<Decoration> {
        self.ws
            .decorations()
            .in_range(buffer, start, end)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Copy buffer-global and matching view-scoped decorations in a range.
    pub fn decorations_in_view(&self, view: ViewId, start: usize, end: usize) -> Vec<Decoration> {
        let Some(buffer) = self.ws.views.get(&view).map(|view| view.buffer_id) else {
            return Vec::new();
        };
        self.ws
            .decorations()
            .in_range_for_view(buffer, view, start, end)
            .into_iter()
            .cloned()
            .collect()
    }

    // --- integration ---

    /// Emit an editor event (drives autocommands).
    pub fn emit(&mut self, event: EditorEvent) {
        self.ws.emit(event);
    }

    /// Request a frontend action (open palette / picker / search).
    pub fn request_ui(&mut self, intent: UiIntent) {
        self.ws.request_ui(intent);
    }

    /// Post a transient notification (toast) with the frontend's default
    /// timeout. The plugin/command-facing `vim.notify` entry point.
    pub fn notify(&mut self, level: NotifyLevel, text: impl Into<String>) {
        self.ws.request_ui(UiIntent::Notify {
            level,
            text: text.into(),
            timeout_ms: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_read_back() {
        let mut ws = Workspace::new();
        let mut api = EditorApi::new(&mut ws);
        assert!(api.insert("abc"));
        assert_eq!(api.line(0).as_deref(), Some("abc"));
        assert_eq!(api.cursor().map(|p| p.col), Some(3));
    }

    #[test]
    fn delete_range_and_undo_redo() {
        let mut ws = Workspace::new();
        let mut api = EditorApi::new(&mut ws);
        api.insert("hello world");
        // delete "hello " -> "world"
        api.delete(Pos::new(0, 0), Pos::new(0, 6));
        assert_eq!(api.line(0).as_deref(), Some("world"));
        assert!(api.undo());
        assert_eq!(api.line(0).as_deref(), Some("hello world"));
        assert!(api.redo());
        assert_eq!(api.line(0).as_deref(), Some("world"));
    }

    #[test]
    fn find_returns_match_offsets() {
        let mut ws = Workspace::new();
        let mut api = EditorApi::new(&mut ws);
        api.insert("ab ab ab");
        assert_eq!(api.find("ab", false), vec![0, 3, 6]);
    }

    #[test]
    fn notify_queues_a_ui_intent() {
        let mut ws = Workspace::new();
        let mut api = EditorApi::new(&mut ws);
        api.notify(NotifyLevel::Error, "boom");
        let intents = ws.drain_ui_intents();
        assert_eq!(
            intents,
            vec![UiIntent::Notify {
                level: NotifyLevel::Error,
                text: "boom".to_string(),
                timeout_ms: None,
            }]
        );
    }

    #[test]
    fn split_and_focus_panes() {
        let mut ws = Workspace::new();
        let mut api = EditorApi::new(&mut ws);
        let first = api.active_view().unwrap();
        let second = api.split(SplitAxis::Vertical).unwrap();
        assert_ne!(first, second);
        assert_eq!(api.active_view(), Some(second));
        assert_eq!(api.focus_previous_pane(), Some(first));
        assert!(api.close_pane()); // closes `first`, promotes the other
    }

    #[test]
    fn decorations_follow_api_undo_and_redo() {
        let mut ws = Workspace::new();
        let mut api = EditorApi::new(&mut ws);
        api.insert("abcd");
        let ns = api.decoration_namespace();
        api.add_decoration(ns, 1, 3, DecorationKind::Highlight(crate::HlRole::Search));

        api.set_cursor(Pos::new(0, 0));
        api.insert("x");
        let bid = api.active_buffer().unwrap();
        assert_eq!(
            api.decorations_in(bid, 0, 10)
                .iter()
                .map(|d| (d.start, d.end))
                .collect::<Vec<_>>(),
            vec![(2, 4)]
        );

        assert!(api.undo());
        assert_eq!(
            api.decorations_in(bid, 0, 10)
                .iter()
                .map(|d| (d.start, d.end))
                .collect::<Vec<_>>(),
            vec![(1, 3)]
        );
        assert!(api.redo());
        assert_eq!(
            api.decorations_in(bid, 0, 10)
                .iter()
                .map(|d| (d.start, d.end))
                .collect::<Vec<_>>(),
            vec![(2, 4)]
        );
    }
}
