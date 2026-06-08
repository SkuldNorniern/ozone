use ozone_buffer::BufferKind;

use crate::events::EditorEvent;
use crate::pane::{FocusDirection, PaneTree, SplitAxis};
use crate::view::ViewId;

use super::Workspace;

impl Workspace {
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
        let target = self
            .panes
            .as_ref()?
            .neighbor_in_direction(current, direction)?;
        self.active_view_id = Some(target);
        Some(target)
    }

    pub fn close_view(&mut self, view_id: ViewId) -> bool {
        if self.views.len() <= 1 {
            return false;
        }

        if matches!(self.panes.as_ref(), Some(PaneTree::Leaf { view_id: pane_view }) if *pane_view == view_id)
        {
            let Some(fallback) = self.views.keys().copied().find(|id| *id != view_id) else {
                return false;
            };
            if let Some(removed) = self.views.remove(&view_id) {
                self.decorations.forget_view(view_id);
                self.remove_unreferenced_virtual_buffer(removed.buffer_id);
            }
            self.panes = Some(PaneTree::leaf(fallback));
            self.active_view_id = Some(fallback);
            return true;
        }

        if let Some(panes) = self.panes.as_mut()
            && panes.remove_leaf(view_id).is_none()
        {
            return false;
        }
        if let Some(removed) = self.views.remove(&view_id) {
            self.decorations.forget_view(view_id);
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

    /// Drop a view that is no longer shown in any pane (e.g. a picker view that
    /// was replaced when its selection opened a file), cleaning up its transient
    /// virtual buffer. No-op if the view is still active or still in the tree.
    pub fn discard_orphan_view(&mut self, view_id: ViewId) {
        if self.active_view_id == Some(view_id) {
            return;
        }
        let in_tree = self
            .panes
            .as_ref()
            .map(|panes| panes.leaves().contains(&view_id))
            .unwrap_or(false);
        if in_tree {
            return;
        }
        if let Some(removed) = self.views.remove(&view_id) {
            self.decorations.forget_view(view_id);
            self.remove_unreferenced_virtual_buffer(removed.buffer_id);
        }
    }

    pub(super) fn show_view_in_active_pane(&mut self, view_id: ViewId) {
        if let (Some(active), Some(panes)) = (self.active_view_id, self.panes.as_mut())
            && panes.replace_leaf(active, view_id)
        {
            self.active_view_id = Some(view_id);
            return;
        }

        self.active_view_id = Some(view_id);
        self.panes = Some(PaneTree::leaf(view_id));
    }

    pub(super) fn remove_unreferenced_virtual_buffer(&mut self, buffer_id: ozone_buffer::BufferId) {
        if self.views.values().any(|view| view.buffer_id == buffer_id) {
            return;
        }

        let should_remove = self
            .buffers
            .get(&buffer_id)
            .map(|buffer| {
                matches!(
                    buffer.kind,
                    BufferKind::Search
                        | BufferKind::References
                        | BufferKind::FileTree
                        | BufferKind::Terminal
                )
            })
            .unwrap_or(false);
        if should_remove {
            if self.buffers.remove(&buffer_id).is_some() {
                self.buffer_options.remove(&buffer_id);
                self.emit(EditorEvent::BufferClosed { id: buffer_id });
            }
        }
    }
}
