use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aurea::render::Rect;
use aurea::{MouseButton, WindowEvent};
use ozone_buffer::{BufferId, BufferKind};
use ozone_editor::{KeyStroke, Workspace};

use crate::actions::{dispatch_autocmds, handle_minibuffer_key, insert_text_raw};
use crate::canvas::SendableCanvas;
use crate::input::{corrected_mods, terminal_key_bytes};
use crate::keys::{Overlays, active_terminal, apply_ui_intents, handle_key};
use crate::layout::{STATUS_H, max_scroll_line, pane_at};
use crate::minibuffer::Minibuffer;
use crate::mouse::{
    MouseState, handle_editor_click, handle_editor_drag, handle_scrollbar_drag,
    handle_scrollbar_press,
};
use crate::notify::Notifications;
use crate::picker::{PickerState, handle_palette_key};
use crate::search::{SearchState, handle_search_key, search_input_text, search_jump};
use crate::terminals::Terminals;
use crate::{ImageCache, OzoneGui, lock};

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
    pub(crate) canvas: Arc<Mutex<SendableCanvas>>,
    pub(crate) last_title: String,
    pub(crate) chord_pending: Vec<KeyStroke>,
    pub(crate) terms: Terminals,
    pub(crate) measured_char_w: f32,
    pub(crate) buffer_mru: Vec<BufferId>,
    pub(crate) images: ImageCache,
    pub(crate) ft_applied: HashSet<BufferId>,
    pub(crate) live_mods: aurea::Modifiers,
    pub(crate) mouse: MouseState,
    pub(crate) cursor_visible: bool,
    pub(crate) last_cursor_blink: Instant,
    pub(crate) needs_redraw: bool,
    pub(crate) window_width: u32,
    pub(crate) window_height: u32,
    cursor_activity: bool,
    swallow_text: bool,
    has_text_input: bool,
}

impl AppState {
    pub(crate) fn new(
        app: OzoneGui,
        palette: Arc<Mutex<Option<PickerState>>>,
        search: Arc<Mutex<Option<SearchState>>>,
        minibuffer: Arc<Mutex<Option<Minibuffer>>>,
        notifications: Arc<Mutex<Notifications>>,
        canvas: Arc<Mutex<SendableCanvas>>,
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
            canvas,
            last_title: String::new(),
            chord_pending: Vec::new(),
            terms: Terminals::new(),
            measured_char_w,
            buffer_mru: Vec::new(),
            images: ImageCache::new(),
            ft_applied: HashSet::new(),
            live_mods: aurea::Modifiers::default(),
            mouse: MouseState::default(),
            cursor_visible: true,
            last_cursor_blink: Instant::now(),
            needs_redraw: false,
            window_width,
            window_height,
            cursor_activity: false,
            swallow_text: false,
            has_text_input: false,
        }
    }

    pub(crate) fn begin_event_batch(&mut self, has_text_input: bool) {
        self.needs_redraw = false;
        self.cursor_activity = false;
        self.swallow_text = false;
        self.has_text_input = has_text_input;
    }

    pub(crate) fn take_cursor_activity(&mut self) -> bool {
        std::mem::take(&mut self.cursor_activity)
    }
}

pub(crate) fn handle_window_event(event: &WindowEvent, state: &mut AppState) -> EventResult {
    match event {
        WindowEvent::CloseRequested => return EventResult::Close,
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
            let mods = corrected_mods(*modifiers, *key, false);
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
            let mods = corrected_mods(*modifiers, *key, true);
            if state.live_mods != mods {
                state.live_mods = mods;
                state.needs_redraw = true;
            }
            state.cursor_activity = true;
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
                    },
                    &state.buffer_mru,
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
                    },
                    &state.buffer_mru,
                ) {
                    state.needs_redraw = true;
                }
            } else if search.is_some() {
                let mut workspace = lock(state.workspace.as_ref());
                if handle_search_key(*key, *modifiers, &mut search, &mut workspace) {
                    dispatch_autocmds(&mut workspace, &state.commands, &state.autocmds);
                    state.needs_redraw = true;
                }
            } else if let Some((term_id, bytes)) = active_terminal(&lock(state.workspace.as_ref()))
                .filter(|id| state.terms.sessions.contains_key(id))
                .and_then(|id| terminal_key_bytes(*key, *modifiers).map(|b| (id, b)))
            {
                state.terms.sessions[&term_id].write_str(bytes);
                state.needs_redraw = true;
            } else {
                if handle_key(
                    *key,
                    *modifiers,
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
                    },
                    &state.buffer_mru,
                ) {
                    state.needs_redraw = true;
                }
                if palette.is_some() || search.is_some() || minibuffer.is_some() {
                    state.swallow_text = true;
                }
            }
        }

        WindowEvent::TextInput { text } => {
            if state.swallow_text {
                state.swallow_text = false;
                return EventResult::Continue;
            }
            state.cursor_activity = true;
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
                        dispatch_autocmds(&mut workspace, &state.commands, &state.autocmds);
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
            {
                let canvas = lock(state.canvas.as_ref());
                let _ = canvas.handle_click(x, y);
            }
            let overlays_open = lock(state.palette.as_ref()).is_some()
                || lock(state.search.as_ref()).is_some()
                || lock(state.minibuffer.as_ref()).is_some();
            if !overlays_open {
                let (width, height) = (state.window_width as f32, state.window_height as f32);
                let mut workspace = lock(state.workspace.as_ref());
                if let Some(press) =
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
