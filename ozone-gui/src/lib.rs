use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use ozone_term::Terminal;

/// Coloured terminal grids by buffer, captured each frame for the renderer.
pub(crate) type TermCells = HashMap<BufferId, Vec<Vec<ozone_term::Cell>>>;

/// Decoded images by buffer. `None` = decode failed (shown as an error label).
pub(crate) type ImageCache = HashMap<BufferId, Option<Image>>;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Decode a PNG/JPEG file into an RGBA8 `Image` for the renderer.
fn decode_image(path: &std::path::Path) -> Option<Image> {
    let rgba = image::open(path).ok()?.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(Image::new(w, h, rgba.into_raw()))
}

use aurea::render::{Canvas, Font, Image, Rect, RendererBackend};
use aurea::{AureaResult, Element, Window, WindowEvent};

mod actions;
mod canvas;
mod components;
mod event;
mod input;
mod keys;
mod layout;
mod minibuffer;
mod mouse;
mod notify;
mod picker;
mod render;
mod search;
mod terminals;
mod theme;
mod whichkey;
pub(crate) use actions::*;
use canvas::{SendableCanvas, SharedCanvas};
use event::{AppState, EventResult, handle_window_event};
use input::*;
use keys::*;
pub(crate) use layout::*;
use minibuffer::*;
use notify::*;
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::Config;
use ozone_editor::commands::register_defaults;
use ozone_editor::{
    AutocommandRegistry, CommandRegistry, IndentConfig, Keymap, ModifierMap, Workspace,
};
use ozone_syntax::Filetype;
use picker::*;
use render::draw_editor;
use search::*;
use terminals::{collect_term_rects, rect_to_grid};
use whichkey::*;

pub struct OzoneGui {
    workspace: Arc<Mutex<Workspace>>,
    commands: Arc<CommandRegistry>,
    config: Arc<Config>,
    autocmds: Arc<AutocommandRegistry>,
    keymap: Arc<Keymap>,
    modmap: ModifierMap,
}

impl OzoneGui {
    pub fn new(workspace: Workspace) -> Self {
        Self::with_config(workspace, Config::default_config())
    }

    pub fn with_config(mut workspace: Workspace, config: Config) -> Self {
        theme::initialize(&config.theme);
        // Editing uses the configured indentation.
        workspace.indent = IndentConfig {
            width: config.editor.tab_width,
            soft_tabs: config.editor.soft_tabs,
        };

        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let autocmds = AutocommandRegistry::from_config(&config.autocmds);
        dispatch_autocmds(&mut workspace, &reg, &autocmds);

        // Layered keymap: shipped defaults, then the user's [[keymap]] on top.
        let mut keymap = Keymap::with_defaults();
        keymap.add_user_config(&config.keymaps);

        // Logical→physical modifier map: platform default + [modifiers] overrides.
        let modmap = ModifierMap::platform_default().with_overrides(
            config.modifiers.control.as_deref(),
            config.modifiers.meta.as_deref(),
            config.modifiers.super_.as_deref(),
        );

        Self {
            workspace: Arc::new(Mutex::new(workspace)),
            commands: Arc::new(reg),
            config: Arc::new(config),
            autocmds: Arc::new(autocmds),
            keymap: Arc::new(keymap),
            modmap,
        }
    }

    pub fn run(self) -> AureaResult<()> {
        const W: u32 = 1280;
        const H: u32 = 800;

        let mut window = Window::new("Ozone", W as i32, H as i32)?;
        set_window_icon(&window);

        // Overlay states shared with the draw callback.
        let palette: Arc<Mutex<Option<PickerState>>> = Arc::new(Mutex::new(None));
        let search: Arc<Mutex<Option<SearchState>>> = Arc::new(Mutex::new(None));
        let minibuffer: Arc<Mutex<Option<Minibuffer>>> = Arc::new(Mutex::new(None));
        // Notification toasts: a single controller owns the list + expiry.
        let notifications: Arc<Mutex<Notifications>> = Arc::new(Mutex::new(Notifications::new()));

        let raw_canvas = Canvas::new(W, H, RendererBackend::Cpu)?;
        let workspace_for_draw = self.workspace.clone();
        let config_for_draw = self.config.clone();
        let palette_for_draw = palette.clone();
        let search_for_draw = search.clone();
        let minibuffer_for_draw = minibuffer.clone();
        let notifications_for_draw = notifications.clone();

        raw_canvas.set_draw_callback(move |ctx| {
            // Keep the same lock order as input handling: overlays, then workspace.
            let pal = lock(palette_for_draw.as_ref());
            let srch = lock(search_for_draw.as_ref());
            let mb = lock(minibuffer_for_draw.as_ref());
            let notes = lock(notifications_for_draw.as_ref());
            let mut ws = lock(workspace_for_draw.as_ref());

            // Repaint callback: terminal colour grids + PTY sizing are driven
            // by the explicit redraw in the run loop, so none here.
            let mut scratch_char_w = 0.0;
            draw_editor(
                ctx,
                &mut ws,
                &config_for_draw,
                srch.as_ref(),
                &TermCells::new(),
                &ImageCache::new(),
                ActiveMods::default(),
                true,
                &mut scratch_char_w,
            )?;
            if let Some(p) = pal.as_ref() {
                draw_palette(ctx, p, &config_for_draw)?;
            }
            if let Some(m) = mb.as_ref() {
                let f = editor_font(&config_for_draw);
                let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                draw_minibuffer(ctx, m, &f, cw, ch, STATUS_H)?;
            }
            if !notes.is_empty() {
                let f = editor_font(&config_for_draw);
                let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                notes.draw(ctx, &f, cw, ch)?;
            }
            Ok(())
        })?;

        let canvas_arc = Arc::new(Mutex::new(SendableCanvas(raw_canvas)));

        // Set canvas directly as window content — no Box wrapper.
        // Keeps the HWND hierarchy as canvas → NativeGuiWindow (one hop).
        // set_window_content resizes the canvas to fill the client area and
        // calls SetFocus(window) so keyboard input works immediately.
        window.set_content(SharedCanvas(canvas_arc.clone()))?;

        {
            let mut canvas = lock(canvas_arc.as_ref());
            let mut ws = lock(self.workspace.as_ref());
            let config = self.config.clone();
            let mut scratch_char_w = 0.0;
            canvas.draw(|ctx| {
                draw_editor(
                    ctx,
                    &mut ws,
                    &config,
                    None,
                    &TermCells::new(),
                    &ImageCache::new(),
                    ActiveMods::default(),
                    true,
                    &mut scratch_char_w,
                )
            })?;
            canvas.invalidate_all();
        }

        let mut state = AppState::new(self, palette, search, minibuffer, notifications, canvas_arc);
        let blink_interval = std::time::Duration::from_millis(530);

        // --------------- event loop ---------------
        loop {
            // Pump Win32 messages FIRST so the events are in the Rust queue
            // before we drain it.  Ensures key presses are processed in the
            // same 8 ms frame they arrive, not the next one.
            unsafe { aurea::ffi::ng_platform_poll_events() };

            // Keep the MRU list current: whichever buffer is active moves to the
            // front. Drop ids whose buffer was closed.
            {
                let ws = lock(state.workspace.as_ref());
                if let Some(active) = ws.active_view().map(|v| v.buffer_id)
                    && state.buffer_mru.first() != Some(&active)
                {
                    state.buffer_mru.retain(|id| *id != active);
                    state.buffer_mru.insert(0, active);
                }
                state.buffer_mru.retain(|id| ws.buffers.contains_key(id));
            }

            let events = window.poll_events();
            let has_text_input = events.iter().any(|event| matches!(event, WindowEvent::TextInput { text } if text.chars().any(|c| !c.is_control())));
            state.begin_event_batch(has_text_input);
            let mut should_close = false;
            for event in &events {
                if matches!(handle_window_event(event, &mut state), EventResult::Close) {
                    should_close = true;
                }
            }
            if should_close {
                break;
            }

            if state.take_cursor_activity() {
                state.cursor_visible = true;
                state.last_cursor_blink = std::time::Instant::now();
            } else if state.last_cursor_blink.elapsed() >= blink_interval {
                state.cursor_visible = !state.cursor_visible;
                state.last_cursor_blink = std::time::Instant::now();
                state.needs_redraw = true;
            }

            // --- terminal sync: spawn, stream output into the buffer, scroll ---
            {
                let mut ws = lock(state.workspace.as_ref());
                let term_bufs: Vec<BufferId> = ws
                    .buffers
                    .iter()
                    .filter(|(_, b)| matches!(b.kind, BufferKind::Terminal))
                    .map(|(id, _)| *id)
                    .collect();
                // Attach a shell to any Terminal buffer that lacks one.
                for id in &term_bufs {
                    if state.terms.sessions.contains_key(id) || state.terms.failed.contains(id) {
                        continue;
                    }
                    match Terminal::spawn() {
                        Ok(term) => {
                            // Size the PTY to the (approx) visible character grid.
                            let cw = (state.config.editor.font_size * 0.6).max(1.0);
                            let lh = (state.config.editor.font_size
                                * state.config.editor.line_height)
                                .max(1.0);
                            let cols = (((W as f32) - 60.0) / cw).clamp(20.0, 500.0) as u16;
                            let rows = (((H as f32) - STATUS_H - EDITOR_TOP_PAD) / lh)
                                .clamp(5.0, 300.0) as u16;
                            term.resize(cols, rows);
                            state.terms.sessions.insert(*id, term);
                        }
                        Err(e) => {
                            state.terms.failed.insert(*id);
                            if let Some(buf) = ws.buffers.get_mut(id) {
                                buf.set_text(&format!("could not start terminal: {e}\n"));
                            }
                            state.needs_redraw = true;
                        }
                    }
                }
                // Forget sessions whose buffer was closed.
                state
                    .terms
                    .sessions
                    .retain(|id, _| ws.buffers.contains_key(id));
                let live_ids = &state.terms.sessions;
                state.terms.cells.retain(|id, _| live_ids.contains_key(id));
                state.terms.sizes.retain(|id, _| live_ids.contains_key(id));

                // Resize each terminal's PTY to match its actual pane rect (a
                // split pane is narrower than the window — otherwise the shell
                // wraps to the full width and overflows the pane).
                let editor_rect = Rect::new(0.0, 0.0, W as f32, (H as f32 - STATUS_H).max(0.0));
                let mut want: Vec<(BufferId, Rect)> = Vec::new();
                if let Some(panes) = &ws.panes {
                    collect_term_rects(&ws, panes, editor_rect, &mut want);
                } else if let Some(bid) = ws.active_view().map(|v| v.buffer_id)
                    && state.terms.sessions.contains_key(&bid)
                {
                    want.push((bid, editor_rect));
                }
                for (bid, rect) in want {
                    let size = rect_to_grid(rect, &state.config, state.measured_char_w);
                    if state.terms.sessions.contains_key(&bid)
                        && state.terms.sizes.get(&bid) != Some(&size)
                    {
                        state.terms.sessions[&bid].resize(size.0, size.1);
                        state.terms.sizes.insert(bid, size);
                    }
                }

                state
                    .terms
                    .versions
                    .retain(|id, _| state.terms.sessions.contains_key(id));
                let active_term = active_terminal(&ws);
                for (id, term) in state.terms.sessions.iter() {
                    let is_active = active_term == Some(*id);
                    // Skip entirely unless the reader thread saw new output (or a
                    // resize bumped the version) — avoids rendering the whole grid
                    // to a string every frame, which TUIs make costly.
                    let version = term.version();
                    if state.terms.versions.get(id) == Some(&version) {
                        continue;
                    }
                    state.terms.versions.insert(*id, version);

                    // The PTY shell echoes input itself, so the buffer is just output.
                    let text = term.output_snapshot();
                    {
                        // Refresh the colour grid alongside the text (same cadence).
                        state.terms.cells.insert(*id, term.cell_snapshot());
                        if let Some(buf) = ws.buffers.get_mut(id) {
                            buf.set_text(&text);
                        }
                        if is_active {
                            // Follow the shell's *own* cursor (not the buffer's last
                            // line). A fresh shell fills only the top of a tall grid,
                            // so pinning to the last line would scroll past all the
                            // content and show blank rows.
                            let (cline, ccol) = term.cursor();
                            let last = ws
                                .buffers
                                .get(id)
                                .map(|b| b.line_count().saturating_sub(1))
                                .unwrap_or(0);
                            let line = cline.min(last);
                            let col = ws
                                .buffers
                                .get(id)
                                .map(|b| b.line_len(line))
                                .unwrap_or(0)
                                .min(ccol);
                            if let Some(view) = ws.active_view_mut() {
                                view.cursor = ozone_buffer::Pos::new(line, col);
                                view.scroll_to_cursor(view.page_height.max(1));
                            }
                        }
                        state.needs_redraw = true;
                    }
                }
            }

            // --- image sync: decode any new image buffer once ---
            {
                let ws = lock(state.workspace.as_ref());
                for (id, buf) in ws.buffers.iter() {
                    if let BufferKind::Image(path) = &buf.kind
                        && !state.images.contains_key(id)
                    {
                        state.images.insert(*id, decode_image(path));
                        state.needs_redraw = true;
                    }
                }
                state.images.retain(|id, _| ws.buffers.contains_key(id));
            }

            // --- filetype-local settings: apply [[filetype]] config once per file ---
            if !state.config.filetypes.is_empty() {
                let mut ws = lock(state.workspace.as_ref());
                let pending: Vec<(BufferId, String)> = ws
                    .buffers
                    .iter()
                    .filter(|(id, _)| !state.ft_applied.contains(id))
                    .filter_map(|(id, b)| match &b.kind {
                        BufferKind::File(p) => Some((
                            *id,
                            filetype_config_name(Filetype::from_path(&p.to_string_lossy())),
                        )),
                        _ => None,
                    })
                    .collect();
                for (id, ftname) in pending {
                    state.ft_applied.insert(id);
                    if let Some(fc) = state.config.filetypes.iter().find(|f| f.name == ftname) {
                        apply_filetype_config(&mut ws, id, fc);
                    }
                }
                state.ft_applied.retain(|id| ws.buffers.contains_key(id));
            }

            // Update window title when active file changes
            // Expire stale notification toasts; repaint when the stack changes.
            if lock(state.notifications.as_ref()).tick() {
                state.needs_redraw = true;
            }

            {
                let ws = lock(state.workspace.as_ref());
                let title = window_title(&ws);
                if title != state.last_title {
                    let _ = window.set_title(&title);
                    state.last_title = title;
                }
            }

            if state.needs_redraw {
                let pal = lock(state.palette.as_ref());
                let srch = lock(state.search.as_ref());
                let mb = lock(state.minibuffer.as_ref());
                let notes = lock(state.notifications.as_ref());
                let mut ws = lock(state.workspace.as_ref());
                let mut canvas = lock(state.canvas.as_ref());
                let config = state.config.clone();
                let active_mods = ActiveMods::from_physical(state.live_mods, &state.modmap);
                // Which-key: while a chord prefix is pending and no other overlay
                // owns the input, show the keys that could come next.
                let (wk_prefix, wk_entries) = if !state.chord_pending.is_empty()
                    && pal.is_none()
                    && srch.is_none()
                    && mb.is_none()
                {
                    let ft = active_filetype_name(&ws);
                    let entries = which_key_entries(
                        &state.keymap,
                        &state.chord_pending,
                        ft.as_deref(),
                        &state.commands,
                    );
                    let prefix = state
                        .chord_pending
                        .iter()
                        .map(ozone_editor::stroke_label)
                        .collect::<Vec<_>>()
                        .join(" ");
                    (prefix, entries)
                } else {
                    (String::new(), Vec::new())
                };
                canvas.draw(|ctx| {
                    draw_editor(
                        ctx,
                        &mut ws,
                        &config,
                        srch.as_ref(),
                        &state.terms.cells,
                        &state.images,
                        active_mods,
                        state.cursor_visible,
                        &mut state.measured_char_w,
                    )?;
                    if let Some(p) = pal.as_ref() {
                        draw_palette(ctx, p, &config)?;
                    }
                    if let Some(m) = mb.as_ref() {
                        let f = editor_font(&config);
                        let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                        draw_minibuffer(ctx, m, &f, cw, ch, STATUS_H)?;
                    }
                    if !wk_entries.is_empty() {
                        let f = editor_font(&config);
                        let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                        draw_which_key(ctx, &wk_prefix, &wk_entries, &f, cw, ch)?;
                    }
                    if !notes.is_empty() {
                        let f = editor_font(&config);
                        let (cw, ch) = (ctx.width() as f32, ctx.height() as f32);
                        notes.draw(ctx, &f, cw, ch)?;
                    }
                    Ok(())
                })?;
                canvas.invalidate_all();
            }

            window.process_frames()?;
            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        Ok(())
    }
}

fn window_title(ws: &Workspace) -> String {
    // Use ASCII-only separators: Windows ANSI title bar can't render em-dashes.
    match ws.active_buffer() {
        Some(buf) => {
            let dirty = if buf.is_dirty() { "*" } else { "" };
            match &buf.kind {
                BufferKind::File(p) | BufferKind::Image(p) => {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    format!("Ozone - {}{}", dirty, name)
                }
                BufferKind::Scratch => format!("Ozone - {}scratch", dirty),
                BufferKind::Search => format!("Ozone - {}files", dirty),
                BufferKind::References => format!("Ozone - {}references", dirty),
                BufferKind::FileTree => format!("Ozone - {}tree", dirty),
                BufferKind::Terminal => format!("Ozone - {}terminal", dirty),
            }
        }
        None => "Ozone".to_string(),
    }
}

pub(crate) fn editor_font(config: &Config) -> Font {
    Font::new(config.editor.font.trim(), config.editor.font_size)
}

fn set_window_icon(window: &Window) {
    let Ok(image) = image::load_from_memory(include_bytes!("../../assets/icon.png")) else {
        return;
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let _ = window.set_icon_rgba(rgba.as_raw(), width, height);
}
