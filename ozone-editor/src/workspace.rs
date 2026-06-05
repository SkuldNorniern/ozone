use std::collections::HashMap;
use std::path::PathBuf;

use ozone_buffer::{Buffer, BufferId, BufferKind};

use crate::events::EditorEvent;
use crate::pane::{FocusDirection, PaneTree, SplitAxis};
use crate::view::{View, ViewId};

/// Top-level state: all buffers and views.
pub struct Workspace {
    pub buffers: HashMap<BufferId, Buffer>,
    pub views: HashMap<ViewId, View>,
    pub active_view_id: Option<ViewId>,
    pub panes: Option<PaneTree>,
    events: Vec<EditorEvent>,
}

impl Workspace {
    pub fn new() -> Self {
        let mut ws = Self {
            buffers: HashMap::new(),
            views: HashMap::new(),
            active_view_id: None,
            panes: None,
            events: Vec::new(),
        };
        // Always have a *scratch* buffer open.
        ws.open_scratch();
        ws
    }

    fn open_scratch(&mut self) {
        let buf = Buffer::new_scratch();
        let buf_id = buf.id;
        self.buffers.insert(buf_id, buf);
        let view = View::new(buf_id);
        let view_id = view.id;
        self.views.insert(view_id, view);
        self.active_view_id = Some(view_id);
        self.panes = Some(PaneTree::leaf(view_id));
    }

    pub fn open_file(&mut self, path: PathBuf) -> std::io::Result<(BufferId, ViewId)> {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let buf = Buffer::open(path)?;
        let buf_id = buf.id;
        let path = match &buf.kind {
            ozone_buffer::BufferKind::File(path) => path.clone(),
            _ => PathBuf::new(),
        };
        self.buffers.insert(buf_id, buf);
        let view = View::new(buf_id);
        let view_id = view.id;
        self.views.insert(view_id, view);
        self.show_view_in_active_pane(view_id);
        self.emit(EditorEvent::BufferOpened { id: buf_id, path: path.clone() });
        self.emit(EditorEvent::BufferFiletype {
            id: buf_id,
            filetype: filetype_name(&path),
        });
        Ok((buf_id, view_id))
    }

    pub fn open_virtual_buffer(&mut self, kind: BufferKind, content: String) -> (BufferId, ViewId) {
        let buf = Buffer::virtual_buffer(kind, &content);
        let buf_id = buf.id;
        self.buffers.insert(buf_id, buf);
        let view = View::new(buf_id);
        let view_id = view.id;
        self.views.insert(view_id, view);
        self.show_view_in_active_pane(view_id);
        (buf_id, view_id)
    }

    pub fn active_view(&self) -> Option<&View> {
        self.active_view_id.and_then(|id| self.views.get(&id))
    }

    pub fn active_view_mut(&mut self) -> Option<&mut View> {
        self.active_view_id.and_then(|id| self.views.get_mut(&id))
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        let view = self.active_view()?;
        self.buffers.get(&view.buffer_id)
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        let id = self.active_view()?.buffer_id;
        self.buffers.get_mut(&id)
    }

    pub fn emit(&mut self, event: EditorEvent) {
        self.events.push(event);
    }

    pub fn drain_events(&mut self) -> Vec<EditorEvent> {
        self.events.drain(..).collect()
    }

    pub fn save_buffer(&mut self, id: BufferId) -> std::io::Result<()> {
        let path = self
            .buffers
            .get(&id)
            .and_then(|buf| match &buf.kind {
                ozone_buffer::BufferKind::File(path) => Some(path.clone()),
                _ => None,
            });

        let Some(buf) = self.buffers.get_mut(&id) else {
            return Ok(());
        };
        buf.save()?;

        if let Some(path) = path {
            self.emit(EditorEvent::BufferSaved { id, path });
        }
        Ok(())
    }

    pub fn split_active_pane(&mut self, axis: SplitAxis) -> Option<ViewId> {
        let active_view_id = self.active_view_id?;
        let new_view = self.views.get(&active_view_id)?.duplicate_for_split();
        let new_view_id = new_view.id;
        self.views.insert(new_view_id, new_view);

        let split = self
            .panes
            .as_mut()
            .map(|panes| panes.split_leaf(active_view_id, new_view_id, axis, 0.5))
            .unwrap_or(false);
        if !split {
            self.panes = Some(PaneTree::leaf(new_view_id));
        }

        self.active_view_id = Some(new_view_id);
        self.emit(EditorEvent::PaneSplit { new_view_id });
        Some(new_view_id)
    }

    pub fn focus_next_pane(&mut self) -> Option<ViewId> {
        let current = self.active_view_id?;
        let next = self.panes.as_ref()?.next_leaf_after(current)?;
        self.active_view_id = Some(next);
        Some(next)
    }

    pub fn focus_previous_pane(&mut self) -> Option<ViewId> {
        let current = self.active_view_id?;
        let previous = self.panes.as_ref()?.previous_leaf_before(current)?;
        self.active_view_id = Some(previous);
        Some(previous)
    }

    pub fn focus_pane_in_direction(&mut self, direction: FocusDirection) -> Option<ViewId> {
        let current = self.active_view_id?;
        let target = self.panes.as_ref()?.neighbor_in_direction(current, direction)?;
        self.active_view_id = Some(target);
        Some(target)
    }

    pub fn close_view(&mut self, view_id: ViewId) -> bool {
        if self.views.len() <= 1 {
            return false;
        }

        if matches!(self.panes.as_ref(), Some(PaneTree::Leaf { view_id: pane_view }) if *pane_view == view_id) {
            let Some(fallback) = self.views.keys().copied().find(|id| *id != view_id) else {
                return false;
            };
            if let Some(removed) = self.views.remove(&view_id) {
                self.remove_unreferenced_virtual_buffer(removed.buffer_id);
            }
            self.panes = Some(PaneTree::leaf(fallback));
            self.active_view_id = Some(fallback);
            return true;
        }

        if let Some(panes) = self.panes.as_mut() {
            if panes.remove_leaf(view_id).is_none() {
                return false;
            }
        }
        if let Some(removed) = self.views.remove(&view_id) {
            self.remove_unreferenced_virtual_buffer(removed.buffer_id);
        }

        if self.active_view_id == Some(view_id) {
            self.active_view_id = self
                .panes
                .as_ref()
                .map(PaneTree::first_leaf)
                .or_else(|| self.views.keys().next().copied());
        }
        true
    }

    fn show_view_in_active_pane(&mut self, view_id: ViewId) {
        if let (Some(active), Some(panes)) = (self.active_view_id, self.panes.as_mut())
            && panes.replace_leaf(active, view_id)
        {
            self.active_view_id = Some(view_id);
            return;
        }

        self.active_view_id = Some(view_id);
        self.panes = Some(PaneTree::leaf(view_id));
    }

    fn remove_unreferenced_virtual_buffer(&mut self, buffer_id: BufferId) {
        if self.views.values().any(|view| view.buffer_id == buffer_id) {
            return;
        }

        let should_remove = self
            .buffers
            .get(&buffer_id)
            .map(|buffer| {
                matches!(
                    buffer.kind,
                    BufferKind::Search | BufferKind::References | BufferKind::Terminal
                )
            })
            .unwrap_or(false);
        if should_remove {
            self.buffers.remove(&buffer_id);
        }
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

fn filetype_name(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "rs" => "rust",
        "toml" => "toml",
        "json" | "jsonc" => "json",
        "md" | "markdown" | "mdown" | "mkd" => "markdown",
        _ => "plain",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_workspace_has_single_pane_for_scratch() {
        let ws = Workspace::new();
        let active = ws.active_view_id.unwrap();
        assert_eq!(ws.panes.as_ref().unwrap().leaves(), vec![active]);
    }

    #[test]
    fn split_active_pane_creates_second_view_on_same_buffer() {
        let mut ws = Workspace::new();
        let original_view = ws.active_view_id.unwrap();
        {
            let view = ws.views.get_mut(&original_view).unwrap();
            view.cursor = ozone_buffer::Pos::new(4, 2);
            view.scroll_line = 3;
        }
        let original = ws.views.get(&original_view).unwrap();
        let original_buffer = original.buffer_id;
        let original_cursor = original.cursor;
        let original_scroll = original.scroll_line;

        let split_view = ws.split_active_pane(SplitAxis::Vertical).unwrap();

        assert_ne!(original_view, split_view);
        assert_eq!(ws.active_view_id, Some(split_view));
        let split = ws.views.get(&split_view).unwrap();
        assert_eq!(split.buffer_id, original_buffer);
        assert_eq!(split.cursor, original_cursor);
        assert_eq!(split.scroll_line, original_scroll);
        assert_eq!(ws.panes.as_ref().unwrap().leaves(), vec![original_view, split_view]);
    }

    #[test]
    fn close_active_split_promotes_remaining_view() {
        let mut ws = Workspace::new();
        let original_view = ws.active_view_id.unwrap();
        let split_view = ws.split_active_pane(SplitAxis::Horizontal).unwrap();

        assert!(ws.close_view(split_view));
        assert_eq!(ws.active_view_id, Some(original_view));
        assert_eq!(ws.panes.as_ref().unwrap().leaves(), vec![original_view]);
    }

    #[test]
    fn opening_virtual_buffer_preserves_previous_view_as_close_fallback() {
        let mut ws = Workspace::new();
        let original = ws.active_view_id.unwrap();

        let (picker_buffer, picker) =
            ws.open_virtual_buffer(BufferKind::Search, "Files\n-----\nPLAN.md\n".to_string());

        assert_eq!(ws.active_view_id, Some(picker));
        assert_eq!(ws.panes.as_ref().unwrap().leaves(), vec![picker]);
        assert!(ws.views.contains_key(&original));

        assert!(ws.close_view(picker));
        assert_eq!(ws.active_view_id, Some(original));
        assert_eq!(ws.panes.as_ref().unwrap().leaves(), vec![original]);
        assert!(!ws.buffers.contains_key(&picker_buffer));
    }

    #[test]
    fn refuses_to_close_last_view() {
        let mut ws = Workspace::new();
        let only_view = ws.active_view_id.unwrap();

        assert!(!ws.close_view(only_view));
        assert_eq!(ws.active_view_id, Some(only_view));
    }

    #[test]
    fn focus_next_and_previous_wrap_between_panes() {
        let mut ws = Workspace::new();
        let first = ws.active_view_id.unwrap();
        let second = ws.split_active_pane(SplitAxis::Vertical).unwrap();
        let third = ws.split_active_pane(SplitAxis::Horizontal).unwrap();

        assert_eq!(ws.active_view_id, Some(third));
        assert_eq!(ws.focus_next_pane(), Some(first));
        assert_eq!(ws.focus_next_pane(), Some(second));
        assert_eq!(ws.focus_previous_pane(), Some(first));
        assert_eq!(ws.focus_previous_pane(), Some(third));
    }
}
