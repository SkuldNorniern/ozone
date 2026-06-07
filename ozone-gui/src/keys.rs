//! Key routing + UI-intent application: how a key event becomes an editor
//! action, and how a command's queued [`UiIntent`] opens the matching overlay.
//!
//! `handle_key` resolves a physical key (through the [`ModifierMap`] + layered
//! [`Keymap`], with chord pending state) into a command, runs it, then drains
//! any `UiIntent` the command queued via `apply_ui_intents`. The editor crate
//! stays windowing-free; this module is the single place the GUI reacts to
//! command-driven overlay requests, so keymaps/palette/autocommands/plugins all
//! drive the same overlays uniformly.

use ozone_buffer::{BufferId, BufferKind};
use ozone_config::FiletypeConfig;
use ozone_editor::{
    AutocommandRegistry, CommandRegistry, KeyStroke, Keymap, KeymapOutcome, ModifierMap, UiIntent,
    Workspace,
};
use ozone_syntax::Filetype;

use crate::actions::{insert_text_raw, run_cmd};
use crate::input::{keycode_to_char, keystroke_from};
use crate::minibuffer::Minibuffer;
use crate::notify::Notifications;
use crate::picker::{
    PickerState, buffer_picker_items, command_picker_items, file_picker_items, select_picker_items,
    theme_picker_items,
};
use crate::search::{SearchState, search_recompute, search_select_from_cursor};
use crate::whichkey::WhichKeyEntry;

/// The mutable overlay state the run loop threads into key routing + intent
/// handling, bundled so it travels as one argument instead of four. Built
/// inline at each call site from the locked overlay guards.
pub(crate) struct Overlays<'a> {
    pub palette: &'a mut Option<PickerState>,
    pub search: &'a mut Option<SearchState>,
    pub minibuffer: &'a mut Option<Minibuffer>,
    pub notifications: &'a mut Notifications,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_key(
    key: aurea::KeyCode,
    mods: aurea::Modifiers,
    allow_text_fallback: bool,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
    keymap: &Keymap,
    modmap: &ModifierMap,
    pending: &mut Vec<KeyStroke>,
    ov: &mut Overlays,
    mru: &[BufferId],
) -> bool {
    use aurea::KeyCode::*;

    // Bare modifier presses are never a binding and never cancel a chord.
    if matches!(key, Shift | Control | Alt | Meta) {
        return false;
    }

    // Picker buffers take precedence so Enter/Esc act on the selection rather
    // than the editing defaults. (Edit keys are swallowed to keep the list.)
    let in_picker = matches!(
        ws.active_buffer().map(|b| &b.kind),
        Some(BufferKind::Search)
    );
    if in_picker && pending.is_empty() && !mods.ctrl && !mods.alt {
        match key {
            Enter => {
                run_cmd("picker.open-selection", ws, reg, autocmds);
                return true;
            }
            Escape => {
                run_cmd("pane.close", ws, reg, autocmds);
                return true;
            }
            Backspace | Delete | Tab => return true,
            _ => {}
        }
    }

    // Resolve through the layered keymap (handles chords via `pending`).
    if let Some(stroke) = keystroke_from(key, mods, modmap) {
        let filetype = active_filetype_name(ws);
        match keymap.resolve(pending, &stroke, filetype.as_deref()) {
            KeymapOutcome::Execute(cmd) => {
                pending.clear();
                // Everything is a command: run it, then perform any frontend
                // action it requested (open palette / picker / search overlay).
                run_cmd(&cmd, ws, reg, autocmds);
                apply_ui_intents(ws, reg, ov, mru);
                return true;
            }
            KeymapOutcome::Pending => {
                pending.push(stroke);
                return true;
            }
            KeymapOutcome::NoMatch => {
                // A failed chord continuation is swallowed; a fresh unmatched key
                // falls through to text entry below.
                let had_pending = !pending.is_empty();
                pending.clear();
                if had_pending {
                    return true;
                }
            }
        }
    }

    // Fallbacks for keys that are not bound commands.
    if !mods.ctrl && !mods.alt {
        if key == Tab {
            let unit = ws.active_indent().unit(); // soft tab / tab (buffer-local aware)
            return insert_text_raw(&unit, ws);
        }
        if allow_text_fallback && let Some(ch) = keycode_to_char(key, mods.shift) {
            let mut buf = [0u8; 4];
            return insert_text_raw(ch.encode_utf8(&mut buf), ws);
        }
    }

    false
}

/// Build which-key panel entries from the keymap's continuations for a pending
/// chord, resolving command ids to friendly display names.
pub(crate) fn which_key_entries(
    keymap: &Keymap,
    pending: &[KeyStroke],
    filetype: Option<&str>,
    reg: &CommandRegistry,
) -> Vec<WhichKeyEntry> {
    keymap
        .continuations(pending, filetype)
        .into_iter()
        .map(|(key, desc)| {
            let is_group = desc == "+prefix";
            let desc = if is_group {
                desc
            } else {
                reg.display_name(&desc)
            };
            WhichKeyEntry {
                key,
                desc,
                is_group,
            }
        })
        .collect()
}

/// The active buffer's id if it is a live terminal surface.
pub(crate) fn active_terminal(ws: &Workspace) -> Option<BufferId> {
    let view = ws.active_view()?;
    match ws.buffers.get(&view.buffer_id)?.kind {
        BufferKind::Terminal => Some(view.buffer_id),
        _ => None,
    }
}

/// Filetype token for the active buffer (for filetype-scoped keymaps).
pub(crate) fn active_filetype_name(ws: &Workspace) -> Option<String> {
    match &ws.active_buffer()?.kind {
        BufferKind::File(p) => Some(filetype_config_name(Filetype::from_path(
            &p.to_string_lossy(),
        ))),
        _ => None,
    }
}

/// Copy a `[[filetype]]` block's set options into a buffer's local overrides.
pub(crate) fn apply_filetype_config(ws: &mut Workspace, id: BufferId, fc: &FiletypeConfig) {
    let local = ws.buffer_local_mut(id);
    if let Some(w) = fc.tab_width {
        local.tab_width = Some(w);
    }
    if let Some(s) = fc.soft_tabs {
        local.soft_tabs = Some(s);
    }
    if let Some(w) = fc.word_wrap {
        local.word_wrap = Some(w);
    }
    if let Some(ln) = fc.line_numbers {
        local.line_numbers = Some(ln);
    }
}

pub(crate) fn filetype_config_name(ft: Filetype) -> String {
    match ft {
        Filetype::Rust => "rust",
        Filetype::Toml => "toml",
        Filetype::Json => "json",
        Filetype::Markdown => "markdown",
        Filetype::Plain => "plain",
    }
    .to_string()
}

/// Perform any [`UiIntent`]s a command queued (open the palette, a picker, or
/// the search bar). Returns whether anything was applied. This is the single
/// place the frontend reacts to command-driven overlay requests, so the editor
/// commands stay GUI-agnostic and plugins can trigger the same overlays.
pub(crate) fn apply_ui_intents(
    ws: &mut Workspace,
    reg: &CommandRegistry,
    ov: &mut Overlays,
    mru: &[BufferId],
) -> bool {
    let intents = ws.drain_ui_intents();
    let applied = !intents.is_empty();
    for intent in intents {
        match intent {
            UiIntent::CommandPalette => {
                *ov.palette = Some(PickerState::new("M-x", command_picker_items(reg)));
            }
            UiIntent::FilePicker => {
                *ov.palette = Some(PickerState::new("find file:", file_picker_items()));
            }
            UiIntent::BufferPicker => {
                let items = buffer_picker_items(ws, mru);
                *ov.palette = Some(PickerState::new("buffer:", items));
            }
            UiIntent::ThemePicker => {
                *ov.palette = Some(PickerState::new("theme:", theme_picker_items()));
            }
            UiIntent::SearchStart => open_search(ws, ov.search, false),
            UiIntent::SearchReplace => open_search(ws, ov.search, true),
            UiIntent::Input { prompt, command } => {
                *ov.minibuffer = Some(Minibuffer::new(prompt, command));
            }
            UiIntent::Select { prompt, items } => {
                *ov.palette = Some(PickerState::new(prompt, select_picker_items(items)));
            }
            UiIntent::Notify {
                level,
                text,
                timeout_ms,
            } => {
                ov.notifications.push(level, text, timeout_ms);
            }
        }
    }
    applied
}

/// Open the search overlay (or reveal the replace field on an open one).
fn open_search(ws: &mut Workspace, search: &mut Option<SearchState>, replace: bool) {
    if let Some(s) = search.as_mut() {
        if replace {
            s.enable_replace();
        }
        return;
    }
    // Record the pre-search position so the jump list can return to it.
    ws.push_jump();
    let mut s = SearchState::new(false);
    if replace {
        s.enable_replace();
    }
    search_recompute(&mut s, ws);
    search_select_from_cursor(&mut s, ws);
    *search = Some(s);
}
