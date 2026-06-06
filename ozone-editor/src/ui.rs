//! UI intents: requests a command makes for the frontend to act on.
//!
//! Some actions — opening the command palette, a fuzzy picker, or the
//! find/replace bar — are *frontend* concerns (they manage overlay widgets and
//! input focus), but they must still be reachable as ordinary commands so they
//! work from keymaps, the palette, autocommands, and (later) plugins.
//!
//! The bridge is a queue: a command calls [`Workspace::request_ui`] with a
//! [`UiIntent`]; the GUI drains the queue each frame and performs the action.
//! This keeps `ozone-editor` free of any windowing dependency while exposing a
//! stable, command-driven extension point for the frontend.

/// A frontend action requested by a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiIntent {
    /// Open the command palette (fuzzy list of all commands).
    CommandPalette,
    /// Open the workspace file picker.
    FilePicker,
    /// Open the open-buffer picker (most-recently-used).
    BufferPicker,
    /// Start incremental in-buffer search.
    SearchStart,
    /// Start in-buffer search with the replace field shown.
    SearchReplace,
    /// Prompt for a line of text in the minibuffer, then run `command` with the
    /// typed text as its argument. The vim.ui.input / Emacs minibuffer pattern;
    /// lets commands (and plugins) take free-form input without a bespoke UI.
    Input { prompt: String, command: String },
    /// Open a fuzzy picker over caller-supplied `items`; choosing one runs its
    /// command (with optional argument). The vim.ui.select pattern — lets any
    /// command or plugin build a custom picker without its own widget.
    Select { prompt: String, items: Vec<SelectItem> },
}

/// One row of a [`UiIntent::Select`] list. Choosing it runs `command` with
/// `arg` (if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    /// Primary text shown in the list.
    pub label: String,
    /// Secondary text (right-aligned, dim); empty to omit.
    pub detail: String,
    /// Command run on commit.
    pub command: String,
    /// Optional argument passed to the command.
    pub arg: Option<String>,
}
