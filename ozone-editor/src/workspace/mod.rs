use std::collections::HashMap;
use std::path::PathBuf;

use ozone_buffer::{Buffer, BufferId, BufferKind, Pos};

use crate::decoration::DecorationStore;
use crate::events::EditorEvent;
use crate::options::{BufferLocal, OptionValue};
use crate::pane::PaneTree;
use crate::ui::UiIntent;
use crate::view::{View, ViewId};

mod nav;
mod pane;

/// A point in the jump list: which buffer and where in it.
pub(super) type Loc = (BufferId, Pos);

/// Maximum remembered jump origins (per direction).
pub(super) const JUMP_CAP: usize = 100;

/// Browser-style back/forward history of cursor jump origins.
#[derive(Default)]
pub(super) struct JumpList {
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
        Self {
            width: 4,
            soft_tabs: true,
        }
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
    pub(super) buffer_options: HashMap<BufferId, BufferLocal>,
    pub(super) decorations: DecorationStore,
    pub(super) events: Vec<EditorEvent>,
    pub(super) ui_intents: Vec<UiIntent>,
    pub(super) jumps: JumpList,
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
        self.push_jump();
        let path = std::fs::canonicalize(&path).unwrap_or(path);
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
        self.emit(EditorEvent::BufferOpened {
            id: buf_id,
            path: path.clone(),
        });
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

    pub fn decorations(&self) -> &DecorationStore {
        &self.decorations
    }

    pub fn decorations_mut(&mut self) -> &mut DecorationStore {
        &mut self.decorations
    }

    /// Publish `diags` for `buffer_id` into `namespace`, replacing any prior set
    /// (see [`crate::diagnostics::publish`]). No-op if the buffer is gone.
    pub fn publish_diagnostics(
        &mut self,
        buffer_id: BufferId,
        namespace: crate::decoration::NamespaceId,
        diags: &[crate::diagnostics::Diagnostic],
    ) {
        if let Some(buf) = self.buffers.get(&buffer_id) {
            crate::diagnostics::publish(&mut self.decorations, buf, buffer_id, namespace, diags);
        }
    }

    pub fn drain_events(&mut self) -> Vec<EditorEvent> {
        self.events.drain(..).collect()
    }

    pub fn request_ui(&mut self, intent: UiIntent) {
        self.ui_intents.push(intent);
    }

    pub fn drain_ui_intents(&mut self) -> Vec<UiIntent> {
        self.ui_intents.drain(..).collect()
    }

    pub fn notify(&mut self, level: crate::ui::NotifyLevel, text: impl Into<String>) {
        self.request_ui(UiIntent::Notify {
            level,
            text: text.into(),
            timeout_ms: None,
        });
    }

    pub fn buffer_local(&self, id: BufferId) -> Option<&BufferLocal> {
        self.buffer_options.get(&id)
    }

    pub fn buffer_local_mut(&mut self, id: BufferId) -> &mut BufferLocal {
        self.buffer_options.entry(id).or_default()
    }

    pub fn set_local(&mut self, id: BufferId, key: &str, value: OptionValue) {
        self.buffer_options.entry(id).or_default().set(key, value);
    }

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

    pub fn active_indent(&self) -> IndentConfig {
        self.active_view()
            .map(|v| self.indent_for(v.buffer_id))
            .unwrap_or(self.indent)
    }

    pub fn save_buffer(&mut self, id: BufferId) -> std::io::Result<()> {
        let path = self.buffers.get(&id).and_then(|buf| match &buf.kind {
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
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a path is a renderable raster image (by extension).
pub fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
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
    use crate::SplitAxis;

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
        assert_eq!(
            ws.panes.as_ref().unwrap().leaves(),
            vec![original_view, split_view]
        );
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
        let mut ws = Workspace::new();
        let scratch = ws.active_buffer().unwrap().id;

        let tmp = std::env::temp_dir().join(format!("ozone_cycle_{}.txt", std::process::id()));
        std::fs::write(&tmp, "alpha\nbeta\n").unwrap();
        let (file_buf, _) = ws.open_file(tmp.clone()).unwrap();

        assert_eq!(ws.active_view().unwrap().buffer_id, file_buf);
        assert!(ws.cycle_buffer(true));
        assert_eq!(ws.active_view().unwrap().buffer_id, scratch);
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
        let (file_buf, _) = ws.open_file(tmp.clone()).unwrap();
        assert_eq!(ws.active_buffer().unwrap().id, file_buf);

        assert!(ws.jump_back());
        assert_eq!(ws.active_buffer().unwrap().id, scratch);
        assert!(ws.jump_forward());
        assert_eq!(ws.active_buffer().unwrap().id, file_buf);
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
        assert!(!ws.switch_active_buffer(scratch));
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
        ws.indent = IndentConfig {
            width: 4,
            soft_tabs: true,
        };
        let bid = ws.active_buffer().unwrap().id;
        assert_eq!(ws.indent_for(bid).width, 4);
        ws.set_local(bid, "tab_width", OptionValue::Int(2));
        ws.set_local(bid, "soft_tabs", OptionValue::Bool(false));
        let eff = ws.indent_for(bid);
        assert_eq!(eff.width, 2);
        assert!(!eff.soft_tabs);
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
        let (buffer, transient) = ws.open_virtual_buffer(BufferKind::Search, "result".to_string());
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
        assert!(
            ws.drain_events()
                .iter()
                .any(|event| matches!(event, EditorEvent::BufferClosed { id } if *id == buffer))
        );
    }
}
