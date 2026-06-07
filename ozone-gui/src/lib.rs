use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::{Arc, Mutex, MutexGuard};

use ozone_term::Terminal;

/// Coloured terminal grids by buffer, captured each frame for the renderer.
pub(crate) type TermCells = HashMap<BufferId, Vec<Vec<ozone_term::Cell>>>;

/// Decoded images by buffer. `None` = decode failed (shown as an error label).
pub(crate) type ImageCache = HashMap<BufferId, Option<Image>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
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
use aurea::{AureaResult, Element, MouseButton, Window, WindowEvent};

mod actions;
mod input;
mod layout;
mod minibuffer;
mod mouse;
mod notify;
mod picker;
mod popup;
mod render;
mod search;
mod theme;
mod whichkey;
pub(crate) use actions::*;
use input::*;
pub(crate) use layout::*;
use minibuffer::*;
use mouse::MouseState;
use notify::*;
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::{Config, FiletypeConfig, LineNumbers};
use ozone_editor::commands::register_defaults;
use ozone_editor::{
    AutocommandRegistry, CommandRegistry, EditorEvent, IndentConfig, KeyStroke,
    Keymap, KeymapOutcome, ModifierMap, PaneTree, UiIntent, Workspace,
};
use ozone_syntax::Filetype;
use picker::*;
use render::draw_editor;
use search::*;
use whichkey::*;

// ---------------------------------------------------------------------------
// SendableCanvas + SharedCanvas wrappers
// ---------------------------------------------------------------------------

struct SendableCanvas(Canvas);
unsafe impl Send for SendableCanvas {}
unsafe impl Sync for SendableCanvas {}
impl std::ops::Deref for SendableCanvas {
    type Target = Canvas;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for SendableCanvas {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Element for SendableCanvas {
    fn handle(&self) -> *mut c_void {
        self.0.handle()
    }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        unsafe { Element::invalidate_platform(&self.0, rect) }
    }
}

struct SharedCanvas(Arc<Mutex<SendableCanvas>>);
impl Element for SharedCanvas {
    fn handle(&self) -> *mut c_void {
        lock(self.0.as_ref()).handle()
    }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        let g = lock(self.0.as_ref());
        unsafe { Element::invalidate_platform(&*g, rect) }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

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
                    &mut scratch_char_w,
                )
            })?;
            canvas.invalidate_all();
        }

        let mut last_title = String::new();
        // Pending chord prefix carried across key events (e.g. after `ctrl+k`).
        let mut chord_pending: Vec<KeyStroke> = Vec::new();
        // Live terminal sessions, keyed by their Terminal buffer.
        let mut terminals: HashMap<BufferId, Terminal> = HashMap::new();
        let mut failed_terminals: std::collections::HashSet<BufferId> =
            std::collections::HashSet::new();
        // Latest coloured grid per terminal, refreshed only when output changes
        // (cloning the whole scrollback every frame would be far too costly).
        let mut term_cells: TermCells = TermCells::new();
        // Last grid size pushed to each terminal's PTY, to avoid redundant resizes.
        let mut term_sizes: HashMap<BufferId, (u16, u16)> = HashMap::new();
        // Last output version seen per terminal; skip re-rendering when unchanged.
        let mut term_versions: HashMap<BufferId, u64> = HashMap::new();
        // Renderer's measured monospace cell width, updated each draw; used to
        // size terminal PTYs so TUIs fill the pane exactly.
        let mut measured_char_w = (self.config.editor.font_size * 0.6).max(1.0);
        // Most-recently-used buffer order (front = current), for the buffer picker.
        let mut buffer_mru: Vec<BufferId> = Vec::new();
        // Decoded images by buffer (decoded once, on first sight).
        let mut images: ImageCache = ImageCache::new();
        // Buffers whose `[[filetype]]` config has already been applied.
        let mut ft_applied: std::collections::HashSet<BufferId> = std::collections::HashSet::new();
        // Live physical modifier state, updated on every key press/release, for
        // the status-bar modifier indicator.
        let mut live_mods = aurea::Modifiers::default();
        // Pointer state for the run loop (last position now; the unified
        // pointer model from docs/aurea-pointer-roadmap.md will grow here).
        let mut mouse = MouseState::default();

        // --------------- event loop ---------------
        loop {
            // Pump Win32 messages FIRST so the events are in the Rust queue
            // before we drain it.  Ensures key presses are processed in the
            // same 8 ms frame they arrive, not the next one.
            unsafe { aurea::ffi::ng_platform_poll_events() };

            // Keep the MRU list current: whichever buffer is active moves to the
            // front. Drop ids whose buffer was closed.
            {
                let ws = lock(self.workspace.as_ref());
                if let Some(active) = ws.active_view().map(|v| v.buffer_id)
                    && buffer_mru.first() != Some(&active)
                {
                    buffer_mru.retain(|id| *id != active);
                    buffer_mru.insert(0, active);
                }
                buffer_mru.retain(|id| ws.buffers.contains_key(id));
            }

            let events = window.poll_events();
            let mut should_close = false;
            let mut needs_redraw = false;
            // When the palette opens via a key, the trigger char (e.g. the `x`
            // of M-x) may also arrive as TextInput; swallow that one char.
            let mut swallow_text = false;

            let has_text_input = events.iter().any(|event| matches!(event, WindowEvent::TextInput { text } if text.chars().any(|c| !c.is_control())));

            for event in events {
                match event {
                    WindowEvent::CloseRequested => {
                        should_close = true;
                    }
                    WindowEvent::Resized { width, height } => {
                        let _ = (width, height);
                        needs_redraw = true;
                    }

                    // Modifier release: refresh the live modifier indicator. The
                    // native modifier snapshot is unreliable for a modifier's own
                    // release (GetKeyState quirk), so derive the bit from the key.
                    WindowEvent::KeyInput {
                        key,
                        pressed: false,
                        modifiers,
                    } => {
                        let m = corrected_mods(modifiers, key, false);
                        if live_mods != m {
                            live_mods = m;
                            needs_redraw = true;
                        }
                    }

                    WindowEvent::KeyInput {
                        key,
                        pressed: true,
                        modifiers,
                    } => {
                        let m = corrected_mods(modifiers, key, true);
                        if live_mods != m {
                            live_mods = m;
                            needs_redraw = true;
                        }
                        let mut pal = lock(palette.as_ref());
                        let mut srch = lock(search.as_ref());
                        let mut mb = lock(minibuffer.as_ref());
                        let mut notes = lock(notifications.as_ref());
                        if mb.is_some() {
                            let mut ws = lock(self.workspace.as_ref());
                            if handle_minibuffer_key(
                                key,
                                &mut mb,
                                &mut ws,
                                &self.commands,
                                &self.autocmds,
                            ) {
                                needs_redraw = true;
                            }
                            // Submitting may queue another intent (e.g. re-prompt).
                            if apply_ui_intents(
                                &mut ws,
                                &self.commands,
                                &mut pal,
                                &mut srch,
                                &mut mb,
                                &mut notes,
                                &buffer_mru,
                            ) {
                                needs_redraw = true;
                            }
                        } else if pal.is_some() {
                            let mut ws = lock(self.workspace.as_ref());
                            if handle_palette_key(
                                key,
                                &mut pal,
                                &mut ws,
                                &self.commands,
                                &self.autocmds,
                            ) {
                                needs_redraw = true;
                            }
                            // A palette selection may itself request an overlay
                            // (e.g. running search.start from the palette).
                            if apply_ui_intents(
                                &mut ws,
                                &self.commands,
                                &mut pal,
                                &mut srch,
                                &mut mb,
                                &mut notes,
                                &buffer_mru,
                            ) {
                                needs_redraw = true;
                            }
                        } else if srch.is_some() {
                            let mut ws = lock(self.workspace.as_ref());
                            if handle_search_key(key, modifiers, &mut srch, &mut ws) {
                                needs_redraw = true;
                            }
                        } else if let Some((term_id, bytes)) =
                            active_terminal(&lock(self.workspace.as_ref()))
                                .filter(|id| terminals.contains_key(id))
                                .and_then(|id| terminal_key_bytes(key, modifiers).map(|b| (id, b)))
                        {
                            // Active buffer is a live terminal and this key maps to
                            // PTY bytes (Enter/Backspace/arrows/Ctrl-C…). Printable
                            // chars come through TextInput; unmapped keys fall to
                            // the editor below.
                            terminals.get(&term_id).unwrap().write_str(bytes);
                            needs_redraw = true;
                        } else {
                            let r = handle_key(
                                key,
                                modifiers,
                                !has_text_input,
                                &mut lock(self.workspace.as_ref()),
                                &self.commands,
                                &self.autocmds,
                                &self.keymap,
                                &self.modmap,
                                &mut chord_pending,
                                &mut pal,
                                &mut srch,
                                &mut mb,
                                &mut notes,
                                &buffer_mru,
                            );
                            if r {
                                needs_redraw = true;
                            }
                            // If a key just opened an overlay, the trigger char
                            // (M-x's `x`, M-g's `g`) also arrives as TextInput on
                            // Windows; drop it so the input starts empty.
                            if pal.is_some() || srch.is_some() || mb.is_some() {
                                swallow_text = true;
                            }
                        }
                    }

                    // Text input is the primary edit path. While the palette is
                    // open, typed chars filter it instead of editing the buffer.
                    WindowEvent::TextInput { text } => {
                        if swallow_text {
                            swallow_text = false; // drop the palette trigger char
                            continue;
                        }
                        let mut mb = lock(minibuffer.as_ref());
                        if let Some(m) = mb.as_mut() {
                            for c in text.chars().filter(|c| !c.is_control()) {
                                m.input.push(c);
                                needs_redraw = true;
                            }
                            continue;
                        }
                        drop(mb);
                        let mut pal = lock(palette.as_ref());
                        if let Some(p) = pal.as_mut() {
                            for c in text.chars().filter(|c| !c.is_control()) {
                                p.push(c);
                                needs_redraw = true;
                            }
                        } else {
                            drop(pal);
                            let mut srch = lock(search.as_ref());
                            if let Some(s) = srch.as_mut() {
                                let mut ws = lock(self.workspace.as_ref());
                                // Typed text edits the query or the replacement,
                                // depending on focus; query edits re-search + jump.
                                if search_input_text(s, &text, &ws) {
                                    if !s.focus_replace {
                                        search_jump(s, &mut ws);
                                    }
                                    needs_redraw = true;
                                }
                            } else {
                                drop(srch);
                                let mut ws = lock(self.workspace.as_ref());
                                let term_id =
                                    active_terminal(&ws).filter(|id| terminals.contains_key(id));
                                if let Some(term_id) = term_id {
                                    // Send typed text straight to the PTY; the shell echoes it back.
                                    let printable: String =
                                        text.chars().filter(|c| !c.is_control()).collect();
                                    if !printable.is_empty() {
                                        terminals.get(&term_id).unwrap().write_str(&printable);
                                        needs_redraw = true;
                                    }
                                } else if insert_text_raw(&text, &mut ws) {
                                    dispatch_autocmds(&mut ws, &self.commands, &self.autocmds);
                                    needs_redraw = true;
                                }
                            }
                        }
                    }

                    WindowEvent::MouseMove { x, y } => {
                        mouse.moved(x as f32, y as f32);
                        if self.config.ui.mouse {
                            let canvas = lock(canvas_arc.as_ref());
                            let _ = canvas.handle_hover(x as f32, y as f32);
                        }
                    }

                    WindowEvent::MouseButton {
                        button: MouseButton::Left,
                        pressed: true,
                        modifiers,
                        x,
                        y,
                        ..
                    } if self.config.ui.mouse => {
                        let (x, y) = (x as f32, y as f32);
                        {
                            // Sparse Canvas controls can opt into Aurea's
                            // InteractiveId callback system.
                            let canvas = lock(canvas_arc.as_ref());
                            let _ = canvas.handle_click(x, y);
                        }
                        // Dense editor text uses coordinate mapping instead of
                        // one interactive display item per glyph.
                        let overlays_open = lock(palette.as_ref()).is_some()
                            || lock(search.as_ref()).is_some()
                            || lock(minibuffer.as_ref()).is_some();
                        if !overlays_open {
                            let (width, height) = {
                                let canvas = lock(canvas_arc.as_ref());
                                (canvas.width() as f32, canvas.height() as f32)
                            };
                            let mut ws = lock(self.workspace.as_ref());
                            if handle_editor_click(
                                &mut ws,
                                &self.config,
                                x,
                                y,
                                width,
                                height,
                                measured_char_w,
                                modifiers.shift,
                            ) {
                                needs_redraw = true;
                            }
                        }
                    }

                    WindowEvent::MouseWheel { delta_y, .. } => {
                        let mut ws = lock(self.workspace.as_ref());
                        if self.config.ui.mouse
                            && let Some((x, y)) = mouse.pos()
                        {
                            let (width, height) = {
                                let canvas = lock(canvas_arc.as_ref());
                                (canvas.width() as f32, canvas.height() as f32)
                            };
                            let editor_rect =
                                Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
                            if let Some((view_id, _)) = ws
                                .panes
                                .as_ref()
                                .and_then(|tree| pane_at(tree, editor_rect, x, y))
                            {
                                ws.active_view_id = Some(view_id);
                            }
                        }
                        let max_scroll = ws
                            .active_view()
                            .and_then(|view| {
                                ws.buffers
                                    .get(&view.buffer_id)
                                    .map(|buf| max_scroll_line(buf.line_count(), view.page_height))
                            })
                            .unwrap_or(0);
                        if let Some(view) = ws.active_view_mut() {
                            let lines = (delta_y.abs() * 3.0).round() as usize;
                            if lines > 0 {
                                if delta_y > 0.0 {
                                    view.scroll_line = view.scroll_line.saturating_sub(lines);
                                } else {
                                    view.scroll_line =
                                        view.scroll_line.saturating_add(lines).min(max_scroll);
                                }
                            }
                        }
                        needs_redraw = true;
                    }

                    _ => {}
                }
            }

            if should_close {
                break;
            }

            // --- terminal sync: spawn, stream output into the buffer, scroll ---
            {
                let mut ws = lock(self.workspace.as_ref());
                let term_bufs: Vec<BufferId> = ws
                    .buffers
                    .iter()
                    .filter(|(_, b)| matches!(b.kind, BufferKind::Terminal))
                    .map(|(id, _)| *id)
                    .collect();
                // Attach a shell to any Terminal buffer that lacks one.
                for id in &term_bufs {
                    if terminals.contains_key(id) || failed_terminals.contains(id) {
                        continue;
                    }
                    match Terminal::spawn() {
                        Ok(term) => {
                            // Size the PTY to the (approx) visible character grid.
                            let cw = (self.config.editor.font_size * 0.6).max(1.0);
                            let lh = (self.config.editor.font_size
                                * self.config.editor.line_height)
                                .max(1.0);
                            let cols = (((W as f32) - 60.0) / cw).clamp(20.0, 500.0) as u16;
                            let rows = (((H as f32) - STATUS_H - EDITOR_TOP_PAD) / lh)
                                .clamp(5.0, 300.0) as u16;
                            term.resize(cols, rows);
                            terminals.insert(*id, term);
                        }
                        Err(e) => {
                            failed_terminals.insert(*id);
                            if let Some(buf) = ws.buffers.get_mut(id) {
                                buf.set_text(&format!("could not start terminal: {e}\n"));
                            }
                            needs_redraw = true;
                        }
                    }
                }
                // Forget sessions whose buffer was closed.
                terminals.retain(|id, _| ws.buffers.contains_key(id));
                term_cells.retain(|id, _| terminals.contains_key(id));
                term_sizes.retain(|id, _| terminals.contains_key(id));

                // Resize each terminal's PTY to match its actual pane rect (a
                // split pane is narrower than the window — otherwise the shell
                // wraps to the full width and overflows the pane).
                let editor_rect = Rect::new(0.0, 0.0, W as f32, (H as f32 - STATUS_H).max(0.0));
                let mut want: Vec<(BufferId, Rect)> = Vec::new();
                if let Some(panes) = &ws.panes {
                    collect_term_rects(&ws, panes, editor_rect, &mut want);
                } else if let Some(bid) = ws.active_view().map(|v| v.buffer_id)
                    && terminals.contains_key(&bid)
                {
                    want.push((bid, editor_rect));
                }
                for (bid, rect) in want {
                    let size = rect_to_grid(rect, &self.config, measured_char_w);
                    if terminals.contains_key(&bid) && term_sizes.get(&bid) != Some(&size) {
                        terminals[&bid].resize(size.0, size.1);
                        term_sizes.insert(bid, size);
                    }
                }

                term_versions.retain(|id, _| terminals.contains_key(id));
                let active_term = active_terminal(&ws);
                for (id, term) in terminals.iter() {
                    let is_active = active_term == Some(*id);
                    // Skip entirely unless the reader thread saw new output (or a
                    // resize bumped the version) — avoids rendering the whole grid
                    // to a string every frame, which TUIs make costly.
                    let version = term.version();
                    if term_versions.get(id) == Some(&version) {
                        continue;
                    }
                    term_versions.insert(*id, version);

                    // The PTY shell echoes input itself, so the buffer is just output.
                    let text = term.output_snapshot();
                    {
                        // Refresh the colour grid alongside the text (same cadence).
                        term_cells.insert(*id, term.cell_snapshot());
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
                        needs_redraw = true;
                    }
                }
            }

            // --- image sync: decode any new image buffer once ---
            {
                let ws = lock(self.workspace.as_ref());
                for (id, buf) in ws.buffers.iter() {
                    if let BufferKind::Image(path) = &buf.kind
                        && !images.contains_key(id)
                    {
                        images.insert(*id, decode_image(path));
                        needs_redraw = true;
                    }
                }
                images.retain(|id, _| ws.buffers.contains_key(id));
            }

            // --- filetype-local settings: apply [[filetype]] config once per file ---
            if !self.config.filetypes.is_empty() {
                let mut ws = lock(self.workspace.as_ref());
                let pending: Vec<(BufferId, String)> = ws
                    .buffers
                    .iter()
                    .filter(|(id, _)| !ft_applied.contains(id))
                    .filter_map(|(id, b)| match &b.kind {
                        BufferKind::File(p) => Some((
                            *id,
                            filetype_config_name(Filetype::from_path(&p.to_string_lossy())),
                        )),
                        _ => None,
                    })
                    .collect();
                for (id, ftname) in pending {
                    ft_applied.insert(id);
                    if let Some(fc) = self.config.filetypes.iter().find(|f| f.name == ftname) {
                        apply_filetype_config(&mut ws, id, fc);
                    }
                }
                ft_applied.retain(|id| ws.buffers.contains_key(id));
            }

            // Update window title when active file changes
            // Expire stale notification toasts; repaint when the stack changes.
            if lock(notifications.as_ref()).tick() {
                needs_redraw = true;
            }

            {
                let ws = lock(self.workspace.as_ref());
                let title = window_title(&ws);
                if title != last_title {
                    let _ = window.set_title(&title);
                    last_title = title;
                }
            }

            if needs_redraw {
                let pal = lock(palette.as_ref());
                let srch = lock(search.as_ref());
                let mb = lock(minibuffer.as_ref());
                let notes = lock(notifications.as_ref());
                let mut ws = lock(self.workspace.as_ref());
                let mut canvas = lock(canvas_arc.as_ref());
                let config = self.config.clone();
                let active_mods = ActiveMods::from_physical(live_mods, &self.modmap);
                // Which-key: while a chord prefix is pending and no other overlay
                // owns the input, show the keys that could come next.
                let (wk_prefix, wk_entries) =
                    if !chord_pending.is_empty() && pal.is_none() && srch.is_none() && mb.is_none()
                    {
                        let ft = active_filetype_name(&ws);
                        let entries = which_key_entries(
                            &self.keymap,
                            &chord_pending,
                            ft.as_deref(),
                            &self.commands,
                        );
                        let prefix = chord_pending
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
                        &term_cells,
                        &images,
                        active_mods,
                        &mut measured_char_w,
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
                BufferKind::Terminal => format!("Ozone - {}terminal", dirty),
            }
        }
        None => "Ozone".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Key routing
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn handle_key(
    key: aurea::KeyCode,
    mods: aurea::Modifiers,
    allow_text_fallback: bool,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
    keymap: &Keymap,
    modmap: &ModifierMap,
    pending: &mut Vec<KeyStroke>,
    palette: &mut Option<PickerState>,
    search: &mut Option<SearchState>,
    minibuffer: &mut Option<Minibuffer>,
    notifications: &mut Notifications,
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
                apply_ui_intents(ws, reg, palette, search, minibuffer, notifications, mru);
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
fn which_key_entries(
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
fn active_terminal(ws: &Workspace) -> Option<BufferId> {
    let view = ws.active_view()?;
    match ws.buffers.get(&view.buffer_id)?.kind {
        BufferKind::Terminal => Some(view.buffer_id),
        _ => None,
    }
}

/// Filetype token for the active buffer (for filetype-scoped keymaps).
fn active_filetype_name(ws: &Workspace) -> Option<String> {
    match &ws.active_buffer()?.kind {
        BufferKind::File(p) => Some(filetype_config_name(Filetype::from_path(
            &p.to_string_lossy(),
        ))),
        _ => None,
    }
}

/// Copy a `[[filetype]]` block's set options into a buffer's local overrides.
fn apply_filetype_config(ws: &mut Workspace, id: BufferId, fc: &FiletypeConfig) {
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

fn filetype_config_name(ft: Filetype) -> String {
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_ui_intents(
    ws: &mut Workspace,
    reg: &CommandRegistry,
    palette: &mut Option<PickerState>,
    search: &mut Option<SearchState>,
    minibuffer: &mut Option<Minibuffer>,
    notifications: &mut Notifications,
    mru: &[BufferId],
) -> bool {
    let intents = ws.drain_ui_intents();
    let applied = !intents.is_empty();
    for intent in intents {
        match intent {
            UiIntent::CommandPalette => {
                *palette = Some(PickerState::new("M-x", command_picker_items(reg)));
            }
            UiIntent::FilePicker => {
                *palette = Some(PickerState::new("find file:", file_picker_items()));
            }
            UiIntent::BufferPicker => {
                let items = buffer_picker_items(ws, mru);
                *palette = Some(PickerState::new("buffer:", items));
            }
            UiIntent::ThemePicker => {
                *palette = Some(PickerState::new("theme:", theme_picker_items()));
            }
            UiIntent::SearchStart => open_search(ws, search, false),
            UiIntent::SearchReplace => open_search(ws, search, true),
            UiIntent::Input { prompt, command } => {
                *minibuffer = Some(Minibuffer::new(prompt, command));
            }
            UiIntent::Select { prompt, items } => {
                *palette = Some(PickerState::new(prompt, select_picker_items(items)));
            }
            UiIntent::Notify {
                level,
                text,
                timeout_ms,
            } => {
                notifications.push(level, text, timeout_ms);
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

pub(crate) fn editor_font(config: &Config) -> Font {
    Font::new(&config.editor.font, config.editor.font_size)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn handle_editor_click(
    ws: &mut Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    char_w: f32,
    extend_selection: bool,
) -> bool {
    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let target = ws
        .panes
        .as_ref()
        .and_then(|tree| pane_at(tree, editor_rect, x, y))
        .or_else(|| ws.active_view_id.map(|id| (id, editor_rect)));
    let Some((view_id, rect)) = target else {
        return false;
    };
    let Some(buffer_id) = ws.views.get(&view_id).map(|view| view.buffer_id) else {
        return false;
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return false;
    };
    if matches!(buf.kind, BufferKind::Image(_) | BufferKind::Terminal) {
        ws.active_view_id = Some(view_id);
        return true;
    }

    let line_count = buf.line_count();
    if line_count == 0 {
        return false;
    }
    let line_numbers = match buf.kind {
        BufferKind::Search | BufferKind::References => LineNumbers::Off,
        _ => ws
            .buffer_local(buffer_id)
            .and_then(|local| local.line_numbers)
            .unwrap_or(config.editor.line_numbers),
    };
    let line_h = (config.editor.font_size * config.editor.line_height).max(1.0);
    let scroll = ws
        .views
        .get(&view_id)
        .map(|view| view.scroll_line)
        .unwrap_or(0);
    let relative_y = (y - rect.y - EDITOR_TOP_PAD).max(0.0);
    let line = (scroll + (relative_y / line_h).floor() as usize).min(line_count - 1);
    let gutter_w = gutter_width(line_count, char_w.max(1.0), line_numbers);
    let text_x = rect.x + gutter_w + PAD;
    let raw_col = if x <= text_x {
        0
    } else {
        ((x - text_x) / char_w.max(1.0)).round() as usize
    };
    let line_text = buf.line(line).unwrap_or_default();
    let mut col = raw_col.min(line_text.len());
    while col > 0 && !line_text.is_char_boundary(col) {
        col -= 1;
    }
    let new_pos = ozone_buffer::Pos::new(line, col);

    let Some(view) = ws.views.get_mut(&view_id) else {
        return false;
    };
    let old_pos = view.cursor;
    view.cursor = new_pos;
    view.col_memory = col;
    view.selection = if extend_selection && old_pos != new_pos {
        let (start, end) = if old_pos <= new_pos {
            (old_pos, new_pos)
        } else {
            (new_pos, old_pos)
        };
        Some(ozone_buffer::Span { start, end })
    } else {
        None
    };
    ws.active_view_id = Some(view_id);
    ws.emit(EditorEvent::CursorMoved {
        view_id,
        pos: new_pos,
    });
    true
}

/// Collect the on-screen rect of every terminal-buffer leaf in a pane tree,
/// mirroring `draw_pane_tree`'s geometry so the PTY can be sized to its pane.
fn collect_term_rects(
    ws: &Workspace,
    tree: &PaneTree,
    rect: Rect,
    out: &mut Vec<(BufferId, Rect)>,
) {
    match tree {
        PaneTree::Leaf { view_id } => {
            if let Some(bid) = ws.views.get(view_id).map(|v| v.buffer_id)
                && matches!(
                    ws.buffers.get(&bid).map(|b| &b.kind),
                    Some(BufferKind::Terminal)
                )
            {
                out.push((bid, rect));
            }
        }
        PaneTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (fr, sr, _) = split_rect(rect, *axis, *ratio);
            collect_term_rects(ws, first, fr, out);
            collect_term_rects(ws, second, sr, out);
        }
    }
}

/// Convert a pane rect to a terminal cell grid `(cols, rows)`, matching the
/// renderer's text origin (left `PAD`, right scrollbar, `EDITOR_TOP_PAD`) and
/// its real measured cell width `char_w`.
fn rect_to_grid(rect: Rect, config: &Config, char_w: f32) -> (u16, u16) {
    let cw = char_w.max(1.0);
    let lh = (config.editor.font_size * config.editor.line_height).max(1.0);
    let usable_w = (rect.width - PAD - 6.0).max(0.0);
    let usable_h = (rect.height - EDITOR_TOP_PAD).max(0.0);
    let cols = (usable_w / cw).clamp(8.0, 1000.0) as u16;
    let rows = (usable_h / lh).clamp(2.0, 1000.0) as u16;
    (cols, rows)
}


#[cfg(test)]
mod mouse_tests {
    use super::*;

    fn text_workspace(text: &str) -> Workspace {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text(text);
        ws
    }

    #[test]
    fn click_places_cursor_using_view_coordinates() {
        let mut ws = text_workspace("alpha\nbravo\ncharlie");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        config.editor.font_size = 10.0;
        config.editor.line_height = 2.0;

        assert!(handle_editor_click(
            &mut ws,
            &config,
            PAD + 3.0 * 8.0,
            EDITOR_TOP_PAD + 1.2 * 20.0,
            800.0,
            600.0,
            8.0,
            false,
        ));
        assert_eq!(
            ws.active_view().unwrap().cursor,
            ozone_buffer::Pos::new(1, 3)
        );
    }

    #[test]
    fn shift_click_creates_ordered_selection() {
        let mut ws = text_workspace("abcdef");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        ws.active_view_mut().unwrap().cursor = ozone_buffer::Pos::new(0, 5);

        assert!(handle_editor_click(
            &mut ws,
            &config,
            PAD + 2.0 * 8.0,
            EDITOR_TOP_PAD,
            800.0,
            600.0,
            8.0,
            true,
        ));
        let selection = ws.active_view().unwrap().selection.unwrap();
        assert_eq!(selection.start, ozone_buffer::Pos::new(0, 2));
        assert_eq!(selection.end, ozone_buffer::Pos::new(0, 5));
    }
}
