use std::collections::HashMap;
use std::path::PathBuf;

use ozone_buffer::{Buffer, BufferId};

use crate::events::EditorEvent;
use crate::pane::{PaneTree, SplitAxis};
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
        self.active_view_id = Some(view_id);
        self.panes = Some(PaneTree::leaf(view_id));
        self.emit(EditorEvent::BufferOpened { id: buf_id, path: path.clone() });
        self.emit(EditorEvent::BufferFiletype {
            id: buf_id,
            filetype: filetype_name(&path),
        });
        Ok((buf_id, view_id))
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
        let buffer_id = self.views.get(&active_view_id)?.buffer_id;
        let new_view = View::new(buffer_id);
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

    pub fn close_view(&mut self, view_id: ViewId) -> bool {
        if self.views.len() <= 1 {
            return false;
        }

        if let Some(panes) = self.panes.as_mut() {
            if panes.remove_leaf(view_id).is_none() {
                return false;
            }
        }
        self.views.remove(&view_id);

        if self.active_view_id == Some(view_id) {
            self.active_view_id = self
                .panes
                .as_ref()
                .map(PaneTree::first_leaf)
                .or_else(|| self.views.keys().next().copied());
        }
        true
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
        let original_buffer = ws.views.get(&original_view).unwrap().buffer_id;

        let split_view = ws.split_active_pane(SplitAxis::Vertical).unwrap();

        assert_ne!(original_view, split_view);
        assert_eq!(ws.active_view_id, Some(split_view));
        assert_eq!(ws.views.get(&split_view).unwrap().buffer_id, original_buffer);
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
    fn refuses_to_close_last_view() {
        let mut ws = Workspace::new();
        let only_view = ws.active_view_id.unwrap();

        assert!(!ws.close_view(only_view));
        assert_eq!(ws.active_view_id, Some(only_view));
    }
}
