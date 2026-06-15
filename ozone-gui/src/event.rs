use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aurea::render::Rect;
use aurea::{MouseButton, WindowEvent};
use ozone_buffer::{BufferId, BufferKind};
use ozone_editor::{KeyStroke, Workspace};

use crate::actions::{apply_auto_save, dispatch_autocmds, handle_minibuffer_key, insert_text_raw};
use crate::canvas::SendableCanvas;
use crate::input::{
    ActiveMods, corrected_mods, keycode_to_char, merge_live_mods, terminal_key_bytes,
};
use crate::keys::{Overlays, active_terminal, apply_ui_intents, handle_key};
use crate::layout::{STATUS_H, max_scroll_line, pane_at};
use crate::lsp::Lsp;
use crate::mouse::{
    MouseState, handle_editor_click, handle_editor_drag, handle_fold_click, handle_scrollbar_drag,
    handle_scrollbar_press,
};
use crate::overlay::completion::{CompletionKeyResult, CompletionState, handle_completion_key};
use crate::overlay::minibuffer::Minibuffer;
use crate::overlay::notify::Notifications;
use crate::overlay::picker::{PickerState, handle_palette_key};
use crate::overlay::search::{SearchState, handle_search_key, search_input_text, search_jump};
use crate::overlay::whichkey::WhichKeyView;
use crate::shell::{
    FileOpenJob, FilePickerJob, FileTreeJob, FolderPickerJob, ShellJobs, WorkspaceSearchJob,
};
use crate::statusbar::buffer_dot_at;
use crate::terminals::Terminals;
use crate::{ImageCache, OzoneGui, SyntaxCache, lock};

pub(crate) enum EventResult {
    Continue,
    Close,
}

pub(crate) struct AppState {
    pub(crate) workspace: Arc<Mutex<Workspace>>,
    pub(crate) commands: Arc<ozone_editor::CommandRegistry>,
    pub(crate) config: Arc<ozone_config::Config>,
    pub(crate) autocmds: Arc<ozone_editor::AutocommandRegistry>,
    pub(crate) keymap: Arc<ozone_editor::Keymap>,
    pub(crate) modmap: ozone_editor::ModifierMap,
    pub(crate) palette: Arc<Mutex<Option<PickerState>>>,
    pub(crate) search: Arc<Mutex<Option<SearchState>>>,
    pub(crate) minibuffer: Arc<Mutex<Option<Minibuffer>>>,
    pub(crate) notifications: Arc<Mutex<Notifications>>,
    /// LSP completion popup, opened from [`crate::lsp::Lsp::take_completion_result`].
    pub(crate) completion: Arc<Mutex<Option<CompletionState>>>,
    /// Which-key view-model shared with the canvas draw callback (the frame the
    /// scheduler actually presents).
    pub(crate) which_key: Arc<Mutex<WhichKeyView>>,
    pub(crate) canvas: Arc<Mutex<SendableCanvas>>,
    pub(crate) last_title: String,
    pub(crate) chord_pending: Vec<KeyStroke>,
    /// When the current `chord_pending` prefix was last extended, for the
    /// idle-chord timeout. `None` while no chord is pending. Paired with
    /// `chord_pending_seen` to restart the clock each time the prefix grows.
    pub(crate) chord_pending_since: Option<Instant>,
    pub(crate) chord_pending_seen: usize,
    pub(crate) terms: Terminals,
    pub(crate) measured_char_w: f32,
    pub(crate) buffer_mru: Vec<BufferId>,
    pub(crate) images: Arc<Mutex<ImageCache>>,
    pub(crate) ft_applied: HashSet<BufferId>,
    pub(crate) live_mods: aurea::Modifiers,
    /// When a bare modifier (Ctrl/Meta) started being held alone, used to delay
    /// the which-key hint so quick chords like `C-s` don't flash it.
    pub(crate) mod_hint_start: Option<Instant>,
    /// Whether the bare-modifier which-key hint is currently shown (tracked to
    /// trigger a redraw only when its visibility flips).
    pub(crate) mod_hint_visible: bool,
    pub(crate) mouse: MouseState,
    /// GUI-side LSP orchestration (lazy server, doc sync, diagnostics routing).
    pub(crate) lsp: Lsp,
    /// In-flight `!cmd` / `|cmd` autocommand jobs (non-blocking).
    pub(crate) shell_jobs: ShellJobs,
    /// In-flight background workspace search, if one is running.
    pub(crate) workspace_search: Option<WorkspaceSearchJob>,
    /// In-flight background file-picker scan, if one is running.
    pub(crate) file_picker_job: Option<FilePickerJob>,
    /// In-flight background file-tree build, if one is running.
    pub(crate) file_tree_job: Option<FileTreeJob>,
    /// In-flight native folder-picker dialog, if one is open.
    pub(crate) folder_picker: Option<FolderPickerJob>,
    /// In-flight native file-picker dialog, if one is open.
    pub(crate) file_open_job: Option<FileOpenJob>,
    pub(crate) syntax_cache: SyntaxCache,
    pub(crate) cursor_visible: bool,
    pub(crate) last_cursor_blink: Instant,
    pub(crate) needs_redraw: bool,
    pub(crate) window_width: u32,
    pub(crate) window_height: u32,
    cursor_activity: bool,
    /// Text-routing gate correlating `KeyInput` with the `TextInput` it
    /// produces. A shortcut key (modified chord, a consumed binding, or a key
    /// that opened an overlay) arms this so the platform-emitted character
    /// byproduct(s) are dropped instead of being typed into the buffer. It
    /// stays armed across *every* `TextInput` until the next non-modifier
    /// `KeyInput` re-arms it (so a key that emits several characters has them
    /// all dropped) or the batch ends. IME-committed text arrives as a
    /// `TextInput` with no preceding shortcut key, so the gate is clear and the
    /// composed string is inserted normally.
    suppress_text_until_next_key: bool,
    has_text_input: bool,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        app: OzoneGui,
        palette: Arc<Mutex<Option<PickerState>>>,
        search: Arc<Mutex<Option<SearchState>>>,
        minibuffer: Arc<Mutex<Option<Minibuffer>>>,
        notifications: Arc<Mutex<Notifications>>,
        completion: Arc<Mutex<Option<CompletionState>>>,
        which_key: Arc<Mutex<WhichKeyView>>,
        canvas: Arc<Mutex<SendableCanvas>>,
        images: Arc<Mutex<ImageCache>>,
        window_width: u32,
        window_height: u32,
    ) -> Self {
        let measured_char_w = (app.config.editor.font_size * 0.6).max(1.0);
        Self {
            workspace: app.workspace,
            commands: app.commands,
            config: app.config,
            autocmds: app.autocmds,
            keymap: app.keymap,
            modmap: app.modmap,
            palette,
            search,
            minibuffer,
            notifications,
            completion,
            which_key,
            canvas,
            last_title: String::new(),
            chord_pending: Vec::new(),
            chord_pending_since: None,
            chord_pending_seen: 0,
            terms: Terminals::new(),
            measured_char_w,
            buffer_mru: Vec::new(),
            images,
            ft_applied: HashSet::new(),
            live_mods: aurea::Modifiers::default(),
            mod_hint_start: None,
            mod_hint_visible: false,
            mouse: MouseState::default(),
            lsp: Lsp::new(),
            shell_jobs: ShellJobs::new(),
            workspace_search: None,
            file_picker_job: None,
            file_tree_job: None,
            folder_picker: None,
            file_open_job: None,
            syntax_cache: SyntaxCache::new(),
            cursor_visible: true,
            last_cursor_blink: Instant::now(),
            needs_redraw: false,
            window_width,
            window_height,
            cursor_activity: false,
            suppress_text_until_next_key: false,
            has_text_input: false,
        }
    }

    pub(crate) fn begin_event_batch(&mut self, has_text_input: bool) {
        self.needs_redraw = false;
        self.cursor_activity = false;
        // A new poll batch: any shortcut-text suppression from the previous
        // batch is stale. (Within a batch a key and its character byproduct
        // arrive together, so suppression never needs to outlive the batch —
        // which keeps IME commits in a later batch from being dropped.)
        self.suppress_text_until_next_key = false;
        self.has_text_input = has_text_input;
    }

    pub(crate) fn take_cursor_activity(&mut self) -> bool {
        std::mem::take(&mut self.cursor_activity)
    }
}

pub(crate) fn handle_window_event(event: &WindowEvent, state: &mut AppState) -> EventResult {
    match event {
        WindowEvent::CloseRequested => return EventResult::Close,

        // Losing focus abandons any half-typed chord: the next stroke could
        // arrive in another app, so a dangling prefix must not survive. Also
        // drop the held-modifier snapshot, which is stale once focus is gone.
        WindowEvent::Unfocused => {
            if !state.chord_pending.is_empty() {
                state.chord_pending.clear();
                state.chord_pending_since = None;
                state.chord_pending_seen = 0;
                state.needs_redraw = true;
            }
            if state.live_mods != aurea::Modifiers::default() {
                state.live_mods = aurea::Modifiers::default();
                state.needs_redraw = true;
            }
        }

        WindowEvent::Resized { width, height } => {
            state.window_width = *width;
            state.window_height = *height;
            state.needs_redraw = true;
        }
        WindowEvent::ScaleFactorChanged { .. } => {
            state.needs_redraw = true;
        }

        // Modifier release: refresh the live modifier indicator. The native
        // modifier snapshot is unreliable for a modifier's own release.
        WindowEvent::KeyInput {
            key,
            pressed: false,
            modifiers,
        } => {
            let snapshot = corrected_mods(*modifiers, *key, false);
            let mods = if matches!(
                *key,
                aurea::KeyCode::Shift
                    | aurea::KeyCode::Control
                    | aurea::KeyCode::Alt
                    | aurea::KeyCode::Meta
            ) {
                snapshot
            } else {
                merge_live_mods(snapshot, state.live_mods)
            };
            if state.live_mods != mods {
                state.live_mods = mods;
                state.needs_redraw = true;
            }
        }

        WindowEvent::KeyInput {
            key,
            pressed: true,
            modifiers,
        } => {
            let snapshot = corrected_mods(*modifiers, *key, true);
            let mods = if matches!(
                *key,
                aurea::KeyCode::Shift
                    | aurea::KeyCode::Control
                    | aurea::KeyCode::Alt
                    | aurea::KeyCode::Meta
            ) {
                snapshot
            } else {
                merge_live_mods(snapshot, state.live_mods)
            };
            if state.live_mods != mods {
                state.live_mods = mods;
                state.needs_redraw = true;
            }
            state.cursor_activity = true;
            let active = ActiveMods::from_physical(mods, &state.modmap);
            // A fresh non-modifier keystroke re-arms text routing: clear any
            // leftover shortcut-text suppression so normal typing after a
            // shortcut isn't dropped. Modifier-only presses belong to a chord
            // and leave the gate alone.
            if !matches!(
                *key,
                aurea::KeyCode::Shift
                    | aurea::KeyCode::Control
                    | aurea::KeyCode::Alt
                    | aurea::KeyCode::Meta
            ) {
                state.suppress_text_until_next_key = false;
            }
            if state.has_text_input
                && keycode_to_char(*key, mods.shift).is_some()
                && (active.control || active.meta || active.super_)
            {
                state.suppress_text_until_next_key = true;
            }

            let mut completion = lock(state.completion.as_ref());
            if completion.is_some() {
                let result = {
                    let mut workspace = lock(state.workspace.as_ref());
                    handle_completion_key(*key, &mut completion, &mut workspace)
                };
                state.needs_redraw = true;
                match result {
                    CompletionKeyResult::Handled => {
                        if matches!(*key, aurea::KeyCode::Enter | aurea::KeyCode::Tab) {
                            let mut workspace = lock(state.workspace.as_ref());
                            dispatch_autocmds(
                                &mut workspace,
                                &state.commands,
                                &state.autocmds,
                                &mut state.shell_jobs,
                            );
                        }
                        return EventResult::Continue;
                    }
                    CompletionKeyResult::Closed => {}
                }
            }
            drop(completion);

            let mut palette = lock(state.palette.as_ref());
            let mut search = lock(state.search.as_ref());
            let mut minibuffer = lock(state.minibuffer.as_ref());
            let mut notifications = lock(state.notifications.as_ref());
            if minibuffer.is_some() {
                let mut workspace = lock(state.workspace.as_ref());
                if handle_minibuffer_key(
                    *key,
                    &mut minibuffer,
                    &mut workspace,
                    &state.commands,
                    &state.autocmds,
                    &mut state.shell_jobs,
                ) {
                    state.needs_redraw = true;
                }
                if apply_ui_intents(
                    &mut workspace,
                    &state.commands,
                    &mut Overlays {
                        palette: &mut palette,
                        search: &mut search,
                        minibuffer: &mut minibuffer,
                        notifications: &mut notifications,
                        workspace_search: &mut state.workspace_search,
                        file_picker_job: &mut state.file_picker_job,
                        file_tree_job: &mut state.file_tree_job,
                        folder_picker: &mut state.folder_picker,
                        file_open_job: &mut state.file_open_job,
                    },
                    &state.buffer_mru,
                    &mut state.lsp,
                ) {
                    state.needs_redraw = true;
                }
            } else if palette.is_some() {
                let mut workspace = lock(state.workspace.as_ref());
                if handle_palette_key(
                    *key,
                    &mut palette,
                    &mut workspace,
                    &state.commands,
                    &state.autocmds,
                    &mut state.shell_jobs,
                ) {
                    state.needs_redraw = true;
                }
                if apply_ui_intents(
                    &mut workspace,
                    &state.commands,
                    &mut Overlays {
                        palette: &mut palette,
                        search: &mut search,
                        minibuffer: &mut minibuffer,
                        notifications: &mut notifications,
                        workspace_search: &mut state.workspace_search,
                        file_picker_job: &mut state.file_picker_job,
                        file_tree_job: &mut state.file_tree_job,
                        folder_picker: &mut state.folder_picker,
                        file_open_job: &mut state.file_open_job,
                    },
                    &state.buffer_mru,
                    &mut state.lsp,
                ) {
                    state.needs_redraw = true;
                }
            } else if search.is_some() {
                let mut workspace = lock(state.workspace.as_ref());
                if handle_search_key(*key, mods, &mut search, &mut workspace) {
                    dispatch_autocmds(
                        &mut workspace,
                        &state.commands,
                        &state.autocmds,
                        &mut state.shell_jobs,
                    );
                    state.needs_redraw = true;
                }
            } else if let Some((term_id, bytes)) = active_terminal(&lock(state.workspace.as_ref()))
                .filter(|id| state.terms.sessions.contains_key(id))
                .zip(terminal_key_bytes(*key, mods))
            {
                state.terms.sessions[&term_id].write_str(bytes);
                state.needs_redraw = true;
            } else {
                let handled = handle_key(
                    *key,
                    mods,
                    !state.has_text_input,
                    &mut lock(state.workspace.as_ref()),
                    &state.commands,
                    &state.autocmds,
                    &state.keymap,
                    &state.modmap,
                    &mut state.chord_pending,
                    &mut Overlays {
                        palette: &mut palette,
                        search: &mut search,
                        minibuffer: &mut minibuffer,
                        notifications: &mut notifications,
                        workspace_search: &mut state.workspace_search,
                        file_picker_job: &mut state.file_picker_job,
                        file_tree_job: &mut state.file_tree_job,
                        folder_picker: &mut state.folder_picker,
                        file_open_job: &mut state.file_open_job,
                    },
                    &state.buffer_mru,
                    &mut state.lsp,
                    &mut state.shell_jobs,
                );
                if handled {
                    state.needs_redraw = true;
                    if state.has_text_input && keycode_to_char(*key, mods.shift).is_some() {
                        state.suppress_text_until_next_key = true;
                    }
                }
                apply_auto_save(
                    &mut lock(state.workspace.as_ref()),
                    &state.config,
                    &state.commands,
                    &state.autocmds,
                    &mut state.shell_jobs,
                );
                if palette.is_some() || search.is_some() || minibuffer.is_some() {
                    state.suppress_text_until_next_key = true;
                }
            }
        }

        WindowEvent::TextInput { text } => {
            // Drop characters that are byproducts of a shortcut key. The gate
            // stays armed (not cleared here) so a key that emits several
            // characters has them all dropped; the next non-modifier KeyInput
            // or the next batch re-arms it.
            if state.suppress_text_until_next_key {
                return EventResult::Continue;
            }
            let active = ActiveMods::from_physical(state.live_mods, &state.modmap);
            if active.control || active.meta || active.super_ {
                return EventResult::Continue;
            }
            state.cursor_activity = true;
            if lock(state.completion.as_ref()).take().is_some() {
                state.needs_redraw = true;
            }
            let mut minibuffer = lock(state.minibuffer.as_ref());
            if let Some(minibuffer) = minibuffer.as_mut() {
                for c in text.chars().filter(|c| !c.is_control()) {
                    minibuffer.input.push(c);
                    state.needs_redraw = true;
                }
                return EventResult::Continue;
            }
            drop(minibuffer);

            let mut palette = lock(state.palette.as_ref());
            if let Some(palette) = palette.as_mut() {
                for c in text.chars().filter(|c| !c.is_control()) {
                    palette.push(c);
                    state.needs_redraw = true;
                }
            } else {
                drop(palette);
                let mut search = lock(state.search.as_ref());
                if let Some(search) = search.as_mut() {
                    let mut workspace = lock(state.workspace.as_ref());
                    if search_input_text(search, text, &mut workspace) {
                        if !search.focus_replace {
                            search_jump(search, &mut workspace);
                        }
                        state.needs_redraw = true;
                    }
                } else {
                    drop(search);
                    let mut workspace = lock(state.workspace.as_ref());
                    let term_id = active_terminal(&workspace)
                        .filter(|id| state.terms.sessions.contains_key(id));
                    if let Some(term_id) = term_id {
                        let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                        if !printable.is_empty() {
                            state.terms.sessions[&term_id].write_str(&printable);
                            state.needs_redraw = true;
                        }
                    } else if insert_text_raw(text, &mut workspace) {
                        dispatch_autocmds(
                            &mut workspace,
                            &state.commands,
                            &state.autocmds,
                            &mut state.shell_jobs,
                        );
                        apply_auto_save(
                            &mut workspace,
                            &state.config,
                            &state.commands,
                            &state.autocmds,
                            &mut state.shell_jobs,
                        );
                        state.needs_redraw = true;
                    }
                }
            }
        }

        WindowEvent::MouseMove { x, y } => {
            let (x, y) = (*x as f32, *y as f32);
            state.mouse.moved(x, y);
            if state.config.ui.mouse {
                if let Some((view_id, grab_y)) = state.mouse.scrollbar_drag() {
                    let (width, height) = (state.window_width as f32, state.window_height as f32);
                    let mut workspace = lock(state.workspace.as_ref());
                    if handle_scrollbar_drag(
                        &mut workspace,
                        &state.config,
                        y,
                        width,
                        height,
                        view_id,
                        grab_y,
                    ) {
                        state.needs_redraw = true;
                        state.cursor_activity = true;
                    }
                } else if let Some((view_id, anchor)) = state.mouse.selection_drag() {
                    let (width, height) = (state.window_width as f32, state.window_height as f32);
                    let mut workspace = lock(state.workspace.as_ref());
                    if handle_editor_drag(
                        &mut workspace,
                        &state.config,
                        x,
                        y,
                        width,
                        height,
                        state.measured_char_w,
                        view_id,
                        anchor,
                    ) {
                        state.needs_redraw = true;
                        state.cursor_activity = true;
                    }
                } else {
                    let canvas = lock(state.canvas.as_ref());
                    let _ = canvas.handle_hover(x, y);
                }
            }
        }

        WindowEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
            modifiers,
            x,
            y,
            click_count,
        } if state.config.ui.mouse => {
            let (x, y) = (*x as f32, *y as f32);
            if lock(state.completion.as_ref()).take().is_some() {
                state.needs_redraw = true;
            }
            {
                let canvas = lock(state.canvas.as_ref());
                let _ = canvas.handle_click(x, y);
            }
            let overlays_open = lock(state.palette.as_ref()).is_some()
                || lock(state.search.as_ref()).is_some()
                || lock(state.minibuffer.as_ref()).is_some();
            if !overlays_open {
                // Use canvas dimensions (same source as rendering) so that dot
                // centers in hit-testing match their drawn positions exactly.
                let (width, height) = {
                    let cv = lock(state.canvas.as_ref());
                    (cv.width() as f32, cv.height() as f32)
                };
                let mut workspace = lock(state.workspace.as_ref());
                if let Some(target) = buffer_dot_at(&workspace, width, height, x, y) {
                    // A status-bar buffer dot: switch to that buffer.
                    if workspace.switch_active_buffer(target) {
                        state.needs_redraw = true;
                    }
                    state.cursor_activity = true;
                } else if handle_fold_click(
                    &mut workspace,
                    &state.config,
                    x,
                    y,
                    width,
                    height,
                    state.measured_char_w,
                ) {
                    state.needs_redraw = true;
                } else if let Some(press) =
                    handle_scrollbar_press(&mut workspace, &state.config, x, y, width, height)
                {
                    state
                        .mouse
                        .begin_scrollbar_drag(press.view_id, press.grab_y);
                    state.needs_redraw |= press.changed;
                    state.cursor_activity = true;
                } else if handle_editor_click(
                    &mut workspace,
                    &state.config,
                    x,
                    y,
                    width,
                    height,
                    state.measured_char_w,
                    modifiers.shift,
                    *click_count,
                ) {
                    let text_like = workspace.active_buffer().is_some_and(|buffer| {
                        !matches!(buffer.kind, BufferKind::Image(_) | BufferKind::Terminal)
                    });
                    if text_like && let Some(view) = workspace.active_view() {
                        let anchor = view
                            .selection
                            .map(|span| {
                                if view.cursor == span.start {
                                    span.end
                                } else {
                                    span.start
                                }
                            })
                            .unwrap_or(view.cursor);
                        state.mouse.begin_selection_drag(view.id, anchor);
                    }
                    state.needs_redraw = true;
                    state.cursor_activity = true;
                }
            }
        }

        WindowEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
            ..
        } if state.config.ui.mouse => {
            state.mouse.end_selection_drag();
            state.mouse.end_scrollbar_drag();
        }

        WindowEvent::MouseWheel { delta_y, .. } => {
            let mut workspace = lock(state.workspace.as_ref());
            if state.config.ui.mouse
                && let Some((x, y)) = state.mouse.pos()
            {
                let (width, height) = (state.window_width as f32, state.window_height as f32);
                let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
                if let Some((view_id, _)) = workspace
                    .panes
                    .as_ref()
                    .and_then(|tree| pane_at(tree, editor_rect, x, y))
                {
                    workspace.active_view_id = Some(view_id);
                }
            }
            let max_scroll = workspace
                .active_view()
                .and_then(|view| {
                    workspace
                        .buffers
                        .get(&view.buffer_id)
                        .map(|buffer| max_scroll_line(buffer.line_count(), view.page_height))
                })
                .unwrap_or(0);
            if let Some(view) = workspace.active_view_mut() {
                let line_h = state.config.editor.font_size * state.config.editor.line_height;
                view.scroll_by_pixels(-(*delta_y as f32) * line_h * 3.0, line_h, max_scroll);
            }
            state.cursor_activity = true;
            state.needs_redraw = true;
        }

        _ => {}
    }

    EventResult::Continue
}
