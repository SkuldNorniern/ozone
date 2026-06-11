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

/// Severity of a [`UiIntent::Notify`] message — drives the toast's colour and,
/// later, filtering/log level. Mirrors the usual `vim.log.levels` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Success,
    Warn,
    Error,
}

/// A frontend action requested by a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiIntent {
    /// Open the command palette (fuzzy list of all commands).
    CommandPalette,
    /// Open the workspace file picker.
    FilePicker,
    /// Open the open-buffer picker (most-recently-used).
    BufferPicker,
    /// Open the installed-theme picker.
    ThemePicker,
    /// Open the document-symbol picker for the active buffer. The frontend
    /// extracts symbols (it owns the syntax layer) and jumps to the chosen one.
    SymbolPicker,
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
    Select {
        prompt: String,
        items: Vec<SelectItem>,
    },
    /// Post a transient notification (toast). `timeout_ms` is how long it stays
    /// before auto-dismissing; `None` uses the frontend default. The vim.notify
    /// / Emacs `message` surface — the frontend owns the popup list and timing.
    Notify {
        level: NotifyLevel,
        text: String,
        timeout_ms: Option<u64>,
    },
    /// Jump to the definition of the symbol under the cursor via the active LSP
    /// server. The frontend owns the connection and resolves the position.
    LspGotoDefinition,
    /// Show hover documentation for the symbol under the cursor via the active
    /// LSP server. The frontend owns the connection and displays the result.
    LspHover,
    /// Request completions at the cursor via the active LSP server. The
    /// frontend owns the connection and shows the results in a popup.
    LspCompletion,
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
