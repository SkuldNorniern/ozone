use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use aurea::render::Font;
use ozone_buffer::BufferId;
use ozone_config::Config;

/// Coloured terminal grids by buffer, captured each frame for the renderer.
pub(crate) type TermCells = HashMap<BufferId, Vec<Vec<ozone_term::Cell>>>;

/// Decoded images by buffer. `None` = decode failed (shown as an error label).
pub(crate) type ImageCache = HashMap<BufferId, Option<aurea::render::Image>>;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn editor_font(config: &Config) -> Font {
    Font::new(config.editor.font.trim(), config.editor.font_size)
}

mod actions;
mod app;
mod canvas;
mod components;
mod event;
mod input;
mod keys;
mod layout;
mod lsp;
mod mouse;
mod overlay;
mod render;
mod shell;
mod statusbar;
mod syntax_cache;
mod terminals;
mod theme;

pub(crate) use syntax_cache::SyntaxCache;

pub use app::OzoneGui;

/// Show a native OS folder-picker dialog and return the selected folder,
/// or `None` if the user cancelled. Blocks until the dialog closes.
/// Call this from the main thread before the editor window opens.
pub fn pick_workspace_folder() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().pick_folder()
}
