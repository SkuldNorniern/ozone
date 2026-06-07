use std::collections::HashMap;
use std::path::PathBuf;

use ozone_buffer::{Buffer, BufferId, BufferKind, Pos};

use crate::decoration::DecorationStore;
use crate::events::EditorEvent;
use crate::options::{BufferLocal, OptionValue};
use crate::pane::{FocusDirection, PaneTree, SplitAxis};
use crate::ui::UiIntent;
use crate::view::{View, ViewId};

/// A point in the jump list: which buffer and where in it.
type Loc = (BufferId, Pos);

/// Maximum remembered jump origins (per direction).
const JUMP_CAP: usize = 100;

/// Browser-style back/forward history of cursor jump origins. `push` records a
/// place you are leaving (clearing the forward stack); `back`/`forward` move
/// between recorded locations.
#[derive(Default)]
struct JumpList {
    back: Vec<Loc>,
    fwd: Vec<Loc>,
}

/// Indentation settings used by editing commands (smart indent, soft tabs).
#[derive(Debug, Clone, Copy)]
pub struct IndentConfig {
    pub width: usize,
    pub soft_tabs: bool,
}

impl Default for IndentConfig {
    fn default() -> Self {
        Self { width: 4, soft_tabs: true }
    }
}

impl IndentConfig {
    /// One indentation level as a string (spaces or a tab).
    pub fn unit(&self) -> String {
        if self.soft_tabs {
            " ".repeat(self.width.max(1))
        } else {
            "\t".to_string()
        }
    }
}

/// Top-level state: all buffers and views.
pub struct Workspace {
    pub buffers: HashMap<BufferId, Buffer>,
    pub views: HashMap<ViewId, View>,
    pub active_view_id: Option<ViewId>,
    pub panes: Option<PaneTree>,
    pub indent: IndentConfig,
    /// Per-buffer setting overrides (`[[filetype]]`, autocommands, plugins).
    buffer_options: HashMap<BufferId, BufferLocal>,
    /// Edit-tracked annotations over buffer ranges (highlights, signs, hints).
    decorations: DecorationStore,
    events: Vec<EditorEvent>,
    ui_intents: Vec<UiIntent>,
    jumps: JumpList,
}

impl Workspace {
    pub fn new() -> Self {
        let mut ws = Self {
            buffers: HashMap::new(),
            views: HashMap::new(),
            active_view_id: None,
            panes: None,
            indent: IndentConfig::default(),
            buffer_options: HashMap::new(),
            decorations: DecorationStore::new(),
            events: Vec::new(),
            ui_intents: Vec::new(),
            jumps: JumpList::default(),
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
        // Opening a file is a jump: record where we were so Ctrl+- returns.
        self.push_jump();
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        // Images render in the pane instead of loading as (binary) text.
        let buf = if is_image_path(&path) {
            Buffer::open_image(path.clone())
        } else {
            Buffer::open(path)?
        };
        let buf_id = buf.id;
        let path = match &buf.kind {
            ozone_buffer::BufferKind::File(path) => path.clone(),
            ozone_buffer::BufferKind::Image(path) => path.clone(),
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
        match &event {
            // Decorations follow edits before observers receive the event.
            EditorEvent::BufferChanged { id, delta } => {
                self.decorations.apply_delta(*id, delta);
            }
            EditorEvent::BufferClosed { id } => {
                self.decorations.forget_buffer(*id);
            }
            _ => {}
        }
        self.events.push(event);
    }

    /// The decoration store (read): highlights, gutter signs, virtual text.
    pub fn decorations(&self) -> &DecorationStore {
        &self.decorations
    }

    /// The decoration store (mutate): add/clear annotations, reserve namespaces.
    pub fn decorations_mut(&mut self) -> &mut DecorationStore {
        &mut self.decorations
    }

    pub fn drain_events(&mut self) -> Vec<EditorEvent> {
        self.events.drain(..).collect()
    }

    /// Queue a frontend action (see [`UiIntent`]). Commands use this to drive
    /// overlays without depending on the GUI; the frontend drains it each frame.
    pub fn request_ui(&mut self, intent: UiIntent) {
        self.ui_intents.push(intent);
    }

    pub fn drain_ui_intents(&mut self) -> Vec<UiIntent> {
        self.ui_intents.drain(..).collect()
    }

    /// Post a transient notification toast with the frontend's default timeout.
    /// Convenience over [`request_ui`](Self::request_ui); see [`EditorApi::notify`].
    pub fn notify(&mut self, level: crate::ui::NotifyLevel, text: impl Into<String>) {
        self.request_ui(UiIntent::Notify { level, text: text.into(), timeout_ms: None });
    }

    /// Buffer-local option overrides for `id`, if any have been set.
    pub fn buffer_local(&self, id: BufferId) -> Option<&BufferLocal> {
        self.buffer_options.get(&id)
    }

    /// Mutable buffer-local overrides for `id`, creating an empty set if needed.
    pub fn buffer_local_mut(&mut self, id: BufferId) -> &mut BufferLocal {
        self.buffer_options.entry(id).or_default()
    }

    /// Set one buffer-local option by name (config / plugin surface).
    pub fn set_local(&mut self, id: BufferId, key: &str, value: OptionValue) {
        self.buffer_options.entry(id).or_default().set(key, value);
    }

    /// Effective indent settings for a buffer: buffer-local overrides layered
    /// over the global default.
    pub fn indent_for(&self, id: BufferId) -> IndentConfig {
        let mut cfg = self.indent;
        if let Some(local) = self.buffer_options.get(&id) {
            if let Some(w) = local.tab_width {
                cfg.width = w;
            }
            if let Some(soft) = local.soft_tabs {
                cfg.soft_tabs = soft;
            }
        }
        cfg
    }

    /// Indent settings for the active buffer (falls back to global).
    pub fn active_indent(&self) -> IndentConfig {
        self.active_view()
            .map(|v| self.indent_for(v.buffer_id))
            .unwrap_or(self.indent)
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

    /// Switch the active pane to the next/previous open buffer (by id order,
    /// wrapping). Repoints the active view and clamps its cursor to the new
    /// buffer. Returns true if the buffer changed.
    pub fn cycle_buffer(&mut self, forward: bool) -> bool {
        // Only cycle real, editable buffers — skip transient picker/terminal surfaces.
        let mut ids: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buf)| matches!(buf.kind, BufferKind::File(_) | BufferKind::Scratch))
            .map(|(id, _)| *id)
            .collect();
        if ids.len() <= 1 {
            return false;
        }
        ids.sort_by_key(|id| id.raw());

        let Some(view_id) = self.active_view_id else {
            return false;
        };
        let Some(current) = self.views.get(&view_id).map(|v| v.buffer_id) else {
            return false;
        };
        let idx = ids.iter().position(|id| *id == current).unwrap_or(0);
        let next = if forward {
            (idx + 1) % ids.len()
        } else {
            (idx + ids.len() - 1) % ids.len()
        };
        let target = ids[next];
        if target == current {
            return false;
        }

        let (last_line, line_len) = {
            let buf = match self.buffers.get(&target) {
                Some(buf) => buf,
                None => return false,
            };
            let last_line = buf.line_count().saturating_sub(1);
            (last_line, buf.line_len(last_line))
        };

        let cursor = if let Some(view) = self.views.get_mut(&view_id) {
            view.buffer_id = target;
            view.cursor.line = view.cursor.line.min(last_line);
            view.cursor.col = view.cursor.col.min(line_len);
            view.col_memory = view.cursor.col;
            view.scroll_line = 0;
            Some((view.id, view.cursor))
        } else {
            None
        };
        if let Some((id, pos)) = cursor {
            self.emit(EditorEvent::CursorMoved { view_id: id, pos });
        }
        true
    }

    /// The active view's current location, if any.
    fn current_loc(&self) -> Option<Loc> {
        let view = self.active_view()?;
        Some((view.buffer_id, view.cursor))
    }

    /// Record the active location as a jump origin (clears the forward stack).
    /// Call before a navigation that should be reachable with `jump_back`.
    pub fn push_jump(&mut self) {
        if let Some(loc) = self.current_loc() {
            if self.jumps.back.last() != Some(&loc) {
                self.jumps.back.push(loc);
                if self.jumps.back.len() > JUMP_CAP {
                    self.jumps.back.remove(0);
                }
            }
            self.jumps.fwd.clear();
        }
    }

    /// Jump to the most recent recorded origin (Ctrl+-). Returns false if there
    /// is nowhere to go back to (skipping origins whose buffer was closed).
    pub fn jump_back(&mut self) -> bool {
        let Some(cur) = self.current_loc() else { return false };
        while let Some(target) = self.jumps.back.pop() {
            if self.buffers.contains_key(&target.0) {
                self.jumps.fwd.push(cur);
                return self.apply_loc(target.0, target.1);
            }
        }
        false
    }

    /// Re-do a jump undone by `jump_back`. Returns false if there is none.
    pub fn jump_forward(&mut self) -> bool {
        let Some(cur) = self.current_loc() else { return false };
        while let Some(target) = self.jumps.fwd.pop() {
            if self.buffers.contains_key(&target.0) {
                self.jumps.back.push(cur);
                return self.apply_loc(target.0, target.1);
            }
        }
        false
    }

    /// Point the active view at `buffer_id`, placing the cursor at `pos`
    /// (clamped) and scrolling it into view. Returns false if the buffer is gone.
    fn apply_loc(&mut self, buffer_id: BufferId, pos: Pos) -> bool {
        let Some(buf) = self.buffers.get(&buffer_id) else { return false };
        let last_line = buf.line_count().saturating_sub(1);
        let line = pos.line.min(last_line);
        let col = pos.col.min(buf.line_len(line));
        let Some(view_id) = self.active_view_id else { return false };
        let Some(view) = self.views.get_mut(&view_id) else { return false };
        view.buffer_id = buffer_id;
        view.cursor = Pos::new(line, col);
        view.col_memory = col;
        view.selection = None;
        view.scroll_to_cursor(view.page_height.max(1));
        let (id, cursor) = (view.id, view.cursor);
        self.emit(EditorEvent::CursorMoved { view_id: id, pos: cursor });
        true
    }

    /// Switch the active pane to a specific open buffer (used by the buffer
    /// picker). Records a jump so Ctrl+- returns to the previous buffer.
    pub fn switch_active_buffer(&mut self, target: BufferId) -> bool {
        if !self.buffers.contains_key(&target) {
            return false;
        }
        if self.current_loc().map(|l| l.0) == Some(target) {
            return false; // already here
        }
        self.push_jump();
        // Land at the top of the target buffer.
        self.apply_loc(target, Pos::zero())
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

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a path is a renderable raster image (by extension).
pub fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
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
    fn closing_view_discards_only_its_scoped_decorations() {
        use crate::decoration::{DecorationKind, HlRole};

        let mut ws = Workspace::new();
        let first = ws.active_view_id.unwrap();
        let buffer = ws.active_buffer().unwrap().id;
        let second = ws.split_active_pane(SplitAxis::Horizontal).unwrap();
        let namespace = ws.decorations_mut().namespace();
        ws.decorations_mut().add_for_view(
            buffer,
            first,
            namespace,
            0,
            1,
            DecorationKind::Highlight(HlRole::Bracket),
        );
        ws.decorations_mut().add_for_view(
            buffer,
            second,
            namespace,
            1,
            2,
            DecorationKind::Highlight(HlRole::Bracket),
        );

        assert!(ws.close_view(second));
        assert_eq!(ws.decorations().all(buffer).len(), 1);
        assert_eq!(ws.decorations().all(buffer)[0].view, Some(first));
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
    fn cycle_buffer_switches_active_view_between_buffers() {
        let mut ws = Workspace::new(); // starts with one scratch buffer
        let scratch = ws.active_buffer().unwrap().id;

        // Open a real file buffer so two editable buffers exist.
        let tmp = std::env::temp_dir().join(format!("ozone_cycle_{}.txt", std::process::id()));
        std::fs::write(&tmp, "alpha\nbeta\n").unwrap();
        let (file_buf, _) = ws.open_file(tmp.clone()).unwrap();

        assert_eq!(ws.active_view().unwrap().buffer_id, file_buf);

        // Cycling forward wraps to scratch (2 editable buffers total).
        assert!(ws.cycle_buffer(true));
        assert_eq!(ws.active_view().unwrap().buffer_id, scratch);

        // Cycling again returns to the file buffer.
        assert!(ws.cycle_buffer(true));
        assert_eq!(ws.active_view().unwrap().buffer_id, file_buf);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn cycle_buffer_noop_with_single_buffer() {
        let mut ws = Workspace::new();
        assert!(!ws.cycle_buffer(true));
    }

    #[test]
    fn switch_active_buffer_records_jump_and_back_returns() {
        let mut ws = Workspace::new();
        let scratch = ws.active_buffer().unwrap().id;

        let tmp = std::env::temp_dir().join(format!("ozone_jump_{}.txt", std::process::id()));
        std::fs::write(&tmp, "alpha\nbeta\n").unwrap();
        // open_file records a jump (scratch) and switches to the file buffer.
        let (file_buf, _) = ws.open_file(tmp.clone()).unwrap();
        assert_eq!(ws.active_buffer().unwrap().id, file_buf);

        // Jump back lands on the scratch buffer again.
        assert!(ws.jump_back());
        assert_eq!(ws.active_buffer().unwrap().id, scratch);

        // Forward returns to the file buffer.
        assert!(ws.jump_forward());
        assert_eq!(ws.active_buffer().unwrap().id, file_buf);

        // No further forward target.
        assert!(!ws.jump_forward());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn switch_active_buffer_changes_buffer_and_is_reversible() {
        let mut ws = Workspace::new();
        let scratch = ws.active_buffer().unwrap().id;
        let tmp = std::env::temp_dir().join(format!("ozone_switch_{}.txt", std::process::id()));
        std::fs::write(&tmp, "x\n").unwrap();
        let (file_buf, _) = ws.open_file(tmp.clone()).unwrap();

        assert!(ws.switch_active_buffer(scratch));
        assert_eq!(ws.active_buffer().unwrap().id, scratch);
        // Switching to the already-active buffer is a no-op.
        assert!(!ws.switch_active_buffer(scratch));
        // The switch recorded a jump back to the file buffer.
        assert!(ws.jump_back());
        assert_eq!(ws.active_buffer().unwrap().id, file_buf);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn jump_back_noop_when_empty() {
        let mut ws = Workspace::new();
        assert!(!ws.jump_back());
        assert!(!ws.jump_forward());
    }

    #[test]
    fn buffer_local_overrides_indent() {
        use crate::options::OptionValue;
        let mut ws = Workspace::new();
        ws.indent = IndentConfig { width: 4, soft_tabs: true };
        let bid = ws.active_buffer().unwrap().id;
        // Global default applies until overridden.
        assert_eq!(ws.indent_for(bid).width, 4);
        ws.set_local(bid, "tab_width", OptionValue::Int(2));
        ws.set_local(bid, "soft_tabs", OptionValue::Bool(false));
        let eff = ws.indent_for(bid);
        assert_eq!(eff.width, 2);
        assert!(!eff.soft_tabs);
        // A second, untouched buffer keeps the global default.
        let other = ozone_buffer::Buffer::from_text("x");
        let oid = other.id;
        ws.buffers.insert(oid, other);
        assert_eq!(ws.indent_for(oid).width, 4);
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

    #[test]
    fn closing_transient_buffer_discards_decorations() {
        use crate::decoration::{DecorationKind, HlRole};

        let mut ws = Workspace::new();
        let original = ws.active_view_id.unwrap();
        let (buffer, transient) =
            ws.open_virtual_buffer(BufferKind::Search, "result".to_string());
        let namespace = ws.decorations_mut().namespace();
        ws.decorations_mut().add(
            buffer,
            namespace,
            0,
            6,
            DecorationKind::Highlight(HlRole::Search),
        );

        ws.show_view_in_active_pane(original);
        ws.discard_orphan_view(transient);

        assert!(!ws.buffers.contains_key(&buffer));
        assert!(ws.decorations().all(buffer).is_empty());
        assert!(ws
            .drain_events()
            .iter()
            .any(|event| matches!(event, EditorEvent::BufferClosed { id } if *id == buffer)));
    }
}
