use std::path::PathBuf;

use ozone_buffer::{BufferId, Delta, Pos};

use crate::view::ViewId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    BufferOpened,
    BufferClosed,
    BufferChanged,
    BufferPreSave,
    BufferSaved,
    BufferFiletype,
    CursorMoved,
    PaneSplit,
    LspAttached,
    LspDiagnostic,
}

impl EventKind {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match normalize_event_name(name).as_str() {
            "buffer.opened" | "buffer.open" => Self::BufferOpened,
            "buffer.closed" | "buffer.close" => Self::BufferClosed,
            "buffer.changed" | "buffer.change" => Self::BufferChanged,
            "buffer.pre-save" | "buffer.presave" | "buffer.pre_save" => Self::BufferPreSave,
            "buffer.saved" | "buffer.save" => Self::BufferSaved,
            "buffer.filetype" | "buffer.file-type" | "buffer.file_type" => Self::BufferFiletype,
            "cursor.moved" | "cursor.move" => Self::CursorMoved,
            "pane.split" => Self::PaneSplit,
            "lsp.attached" | "lsp.attach" => Self::LspAttached,
            "lsp.diagnostic" | "lsp.diagnostics" => Self::LspDiagnostic,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::BufferOpened => "buffer.opened",
            Self::BufferClosed => "buffer.closed",
            Self::BufferChanged => "buffer.changed",
            Self::BufferPreSave => "buffer.pre-save",
            Self::BufferSaved => "buffer.saved",
            Self::BufferFiletype => "buffer.filetype",
            Self::CursorMoved => "cursor.moved",
            Self::PaneSplit => "pane.split",
            Self::LspAttached => "lsp.attached",
            Self::LspDiagnostic => "lsp.diagnostic",
        }
    }
}

#[derive(Debug, Clone)]
pub enum EditorEvent {
    BufferOpened { id: BufferId, path: PathBuf },
    BufferClosed { id: BufferId },
    BufferChanged { id: BufferId, delta: Delta },
    BufferPreSave { id: BufferId, path: PathBuf },
    BufferSaved { id: BufferId, path: PathBuf },
    BufferFiletype { id: BufferId, filetype: String },
    CursorMoved { view_id: ViewId, pos: Pos },
    PaneSplit { new_view_id: ViewId },
    LspAttached { buffer_id: BufferId, server: String },
    LspDiagnostic { buffer_id: BufferId, count: usize },
}

impl EditorEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::BufferOpened { .. } => EventKind::BufferOpened,
            Self::BufferClosed { .. } => EventKind::BufferClosed,
            Self::BufferChanged { .. } => EventKind::BufferChanged,
            Self::BufferPreSave { .. } => EventKind::BufferPreSave,
            Self::BufferSaved { .. } => EventKind::BufferSaved,
            Self::BufferFiletype { .. } => EventKind::BufferFiletype,
            Self::CursorMoved { .. } => EventKind::CursorMoved,
            Self::PaneSplit { .. } => EventKind::PaneSplit,
            Self::LspAttached { .. } => EventKind::LspAttached,
            Self::LspDiagnostic { .. } => EventKind::LspDiagnostic,
        }
    }

    /// Borrowed text autocommand patterns match against, without cloning the
    /// event's owned path/filetype/server string.
    pub fn match_text(&self) -> Option<&str> {
        match self {
            Self::BufferOpened { path, .. }
            | Self::BufferPreSave { path, .. }
            | Self::BufferSaved { path, .. } => path.to_str(),
            Self::BufferFiletype { filetype, .. } => Some(filetype.as_str()),
            Self::LspAttached { server, .. } => Some(server.as_str()),
            _ => None,
        }
    }
}

fn normalize_event_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('_', "-")
}
