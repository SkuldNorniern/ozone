use std::os::raw::c_void;
use std::sync::{Arc, Mutex};

use aurea::render::{Canvas, Color, DrawingContext, Font, Paint, PaintStyle, Point, Rect, RendererBackend};
use aurea::{AureaResult, Element, Window, WindowEvent};
use ozone_buffer::{BufferId, BufferKind};
use ozone_editor::{AutocommandRegistry, CommandContext, CommandRegistry, EditorEvent, KeyStroke, Keymap, KeymapOutcome, PaneTree, SplitAxis, ViewId, Workspace};
use ozone_editor::commands::register_defaults;
use ozone_syntax::{Filetype, ScanState, TokenKind, scan_line};
use ozone_config::{Config, CursorStyle, LineNumbers};

// ---------------------------------------------------------------------------
// SendableCanvas + SharedCanvas wrappers
// ---------------------------------------------------------------------------

struct SendableCanvas(Canvas);
unsafe impl Send for SendableCanvas {}
unsafe impl Sync for SendableCanvas {}
impl std::ops::Deref for SendableCanvas {
    type Target = Canvas;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for SendableCanvas {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}
impl Element for SendableCanvas {
    fn handle(&self) -> *mut c_void { self.0.handle() }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        unsafe { Element::invalidate_platform(&self.0, rect) }
    }
}

struct SharedCanvas(Arc<Mutex<SendableCanvas>>);
impl Element for SharedCanvas {
    fn handle(&self) -> *mut c_void { self.0.lock().unwrap().handle() }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        let g = self.0.lock().unwrap();
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
}

impl OzoneGui {
    pub fn new(workspace: Workspace) -> Self {
        Self::with_config(workspace, Config::default_config())
    }

    pub fn with_config(mut workspace: Workspace, config: Config) -> Self {
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let autocmds = AutocommandRegistry::from_config(&config.autocmds);
        dispatch_autocmds(&mut workspace, &reg, &autocmds);

        // Layered keymap: shipped defaults, then the user's [[keymap]] on top.
        let mut keymap = Keymap::with_defaults();
        keymap.add_user_config(&config.keymaps);

        Self {
            workspace: Arc::new(Mutex::new(workspace)),
            commands: Arc::new(reg),
            config: Arc::new(config),
            autocmds: Arc::new(autocmds),
            keymap: Arc::new(keymap),
        }
    }

    pub fn run(self) -> AureaResult<()> {
        const W: u32 = 1280;
        const H: u32 = 800;

        let mut window = Window::new("Ozone", W as i32, H as i32)?;

        let raw_canvas = Canvas::new(W, H, RendererBackend::Cpu)?;
        let workspace_for_draw = self.workspace.clone();
        let config_for_draw = self.config.clone();

        raw_canvas.set_draw_callback(move |ctx| {
            let mut ws = workspace_for_draw.lock().unwrap();
            draw_editor(ctx, &mut ws, &config_for_draw)
        })?;

        let canvas_arc = Arc::new(Mutex::new(SendableCanvas(raw_canvas)));

        // Set canvas directly as window content — no Box wrapper.
        // Keeps the HWND hierarchy as canvas → NativeGuiWindow (one hop).
        // set_window_content resizes the canvas to fill the client area and
        // calls SetFocus(window) so keyboard input works immediately.
        window.set_content(SharedCanvas(canvas_arc.clone()))?;

        canvas_arc.lock().unwrap().invalidate_all();

        let mut last_title = String::new();
        // Pending chord prefix carried across key events (e.g. after `ctrl+k`).
        let mut chord_pending: Vec<KeyStroke> = Vec::new();

        // --------------- event loop ---------------
        loop {
            // Pump Win32 messages FIRST so the events are in the Rust queue
            // before we drain it.  Ensures key presses are processed in the
            // same 8 ms frame they arrive, not the next one.
            unsafe { aurea::ffi::ng_platform_poll_events() };

            let events = window.poll_events();
            let mut should_close = false;
            let mut needs_redraw = false;

            let has_text_input = events.iter().any(|event| {
                matches!(event, WindowEvent::TextInput { text } if text.chars().any(|c| !c.is_control()))
            });

            for event in events {
                match event {
                    WindowEvent::CloseRequested => { should_close = true; }
                    WindowEvent::Resized { width, height } => {
                        let _ = (width, height);
                        needs_redraw = true;
                    }

                    WindowEvent::KeyInput { key, pressed: true, modifiers } => {
                        if handle_key(
                            key,
                            modifiers,
                            !has_text_input,
                            &mut self.workspace.lock().unwrap(),
                            &self.commands,
                            &self.autocmds,
                            &self.keymap,
                            &mut chord_pending,
                        ) {
                            needs_redraw = true;
                        }
                    }

                    // Text input is the primary edit path. KeyInput handles commands
                    // and provides a simple ASCII fallback for backends without WM_CHAR.
                    WindowEvent::TextInput { text } => {
                        let mut ws = self.workspace.lock().unwrap();
                        if insert_text_raw(&text, &mut ws) {
                            dispatch_autocmds(&mut ws, &self.commands, &self.autocmds);
                            needs_redraw = true;
                        }
                    }

                    WindowEvent::MouseWheel { delta_y, .. } => {
                        let mut ws = self.workspace.lock().unwrap();
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
                                    view.scroll_line = view.scroll_line.saturating_add(lines).min(max_scroll);
                                }
                            }
                        }
                        needs_redraw = true;
                    }

                    _ => {}
                }
            }

            if should_close { break; }

            // Update window title when active file changes
            {
                let ws = self.workspace.lock().unwrap();
                let title = window_title(&ws);
                if title != last_title {
                    let _ = window.set_title(&title);
                    last_title = title;
                }
            }

            if needs_redraw {
                let mut canvas = canvas_arc.lock().unwrap();
                let mut ws = self.workspace.lock().unwrap();
                let config = self.config.clone();
                canvas.draw(|ctx| draw_editor(ctx, &mut ws, &config))?;
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
                BufferKind::File(p) => {
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
    pending: &mut Vec<KeyStroke>,
) -> bool {
    use aurea::KeyCode::*;

    // Bare modifier presses are never a binding and never cancel a chord.
    if matches!(key, Shift | Control | Alt | Meta) {
        return false;
    }

    // Picker buffers take precedence so Enter/Esc act on the selection rather
    // than the editing defaults. (Edit keys are swallowed to keep the list.)
    let in_picker = matches!(ws.active_buffer().map(|b| &b.kind), Some(BufferKind::Search));
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
    if let Some(stroke) = keystroke_from(key, mods) {
        let filetype = active_filetype_name(ws);
        match keymap.resolve(pending, &stroke, filetype.as_deref()) {
            KeymapOutcome::Execute(cmd) => {
                pending.clear();
                run_cmd(&cmd, ws, reg, autocmds);
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
            return insert_text_raw("    ", ws); // soft tab
        }
        if allow_text_fallback && let Some(ch) = keycode_to_char(key, mods.shift) {
            let mut buf = [0u8; 4];
            return insert_text_raw(ch.encode_utf8(&mut buf), ws);
        }
    }

    false
}

/// Filetype token for the active buffer (for filetype-scoped keymaps).
fn active_filetype_name(ws: &Workspace) -> Option<String> {
    match &ws.active_buffer()?.kind {
        BufferKind::File(p) => Some(filetype_config_name(Filetype::from_path(&p.to_string_lossy()))),
        _ => None,
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

/// Convert a platform key + modifiers into a normalized [`KeyStroke`].
/// Returns `None` for keys with no token (modifiers, unknown codes).
fn keystroke_from(key: aurea::KeyCode, mods: aurea::Modifiers) -> Option<KeyStroke> {
    let token = keycode_token(key)?;
    Some(KeyStroke {
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
        meta: mods.meta,
        key: token.to_string(),
    })
}

/// The normalized keymap token for a key code (lowercase, matches config parsing).
fn keycode_token(key: aurea::KeyCode) -> Option<&'static str> {
    use aurea::KeyCode::*;
    Some(match key {
        A => "a", B => "b", C => "c", D => "d", E => "e", F => "f", G => "g",
        H => "h", I => "i", J => "j", K => "k", L => "l", M => "m", N => "n",
        O => "o", P => "p", Q => "q", R => "r", S => "s", T => "t", U => "u",
        V => "v", W => "w", X => "x", Y => "y", Z => "z",
        Key0 => "0", Key1 => "1", Key2 => "2", Key3 => "3", Key4 => "4",
        Key5 => "5", Key6 => "6", Key7 => "7", Key8 => "8", Key9 => "9",
        Space => "space", Enter => "enter", Escape => "escape", Tab => "tab",
        Backspace => "backspace", Delete => "delete", Insert => "insert",
        Home => "home", End => "end", PageUp => "pageup", PageDown => "pagedown",
        Up => "up", Down => "down", Left => "left", Right => "right",
        F1 => "f1", F2 => "f2", F3 => "f3", F4 => "f4", F5 => "f5", F6 => "f6",
        F7 => "f7", F8 => "f8", F9 => "f9", F10 => "f10", F11 => "f11", F12 => "f12",
        Shift | Control | Alt | Meta | Unknown(_) => return None,
    })
}

fn run_cmd(name: &str, ws: &mut Workspace, reg: &CommandRegistry, autocmds: &AutocommandRegistry) {
    if name == "file.save" {
        if let Some(buffer_id) = ws.active_view().map(|view| view.buffer_id) {
            run_pre_save_autocmds(buffer_id, ws, reg, autocmds);
        }
    } else if name == "file.save-all" {
        let ids: Vec<_> = ws.buffers.keys().copied().collect();
        for id in ids {
            run_pre_save_autocmds(id, ws, reg, autocmds);
        }
    }

    execute_command(name, ws, reg);
    dispatch_autocmds(ws, reg, autocmds);
}

fn execute_command(name: &str, ws: &mut Workspace, reg: &CommandRegistry) {
    if let Some(mut ctx) = CommandContext::new(ws) {
        reg.execute(name, &mut ctx);
    }
    if let Some(view) = ws.active_view_mut() {
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

fn run_pre_save_autocmds(
    buffer_id: BufferId,
    ws: &mut Workspace,
    reg: &CommandRegistry,
    autocmds: &AutocommandRegistry,
) {
    let path = ws.buffers.get(&buffer_id).and_then(|buf| match &buf.kind {
        BufferKind::File(path) => Some(path.clone()),
        _ => None,
    });
    let Some(path) = path else {
        return;
    };

    let event = EditorEvent::BufferPreSave { id: buffer_id, path };
    let commands: Vec<String> = autocmds
        .matching_commands(&event)
        .into_iter()
        .map(str::to_string)
        .collect();
    for command in commands {
        if command == "file.save" || command == "file.save-all" {
            continue;
        }
        execute_command(&command, ws, reg);
    }
}

fn dispatch_autocmds(ws: &mut Workspace, reg: &CommandRegistry, autocmds: &AutocommandRegistry) {
    const MAX_AUTOCMD_ROUNDS: usize = 16;

    for _ in 0..MAX_AUTOCMD_ROUNDS {
        let events = ws.drain_events();
        if events.is_empty() {
            break;
        }

        let commands: Vec<String> = events
            .iter()
            .flat_map(|event| autocmds.matching_commands(event))
            .map(str::to_string)
            .collect();

        if commands.is_empty() {
            continue;
        }

        for command in commands {
            if command == "file.save" || command == "file.save-all" {
                continue;
            }
            execute_command(&command, ws, reg);
        }
    }
}

fn insert_text_raw(text: &str, ws: &mut Workspace) -> bool {
    let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
    if filtered.is_empty() { return false; }

    let Some(view) = ws.active_view() else { return false };
    let cursor = view.cursor;
    let buf_id = view.buffer_id;

    // Virtual/read-only surfaces (pickers, terminal placeholder) reject edits.
    if matches!(
        ws.buffers.get(&buf_id).map(|b| &b.kind),
        Some(BufferKind::Search | BufferKind::References | BufferKind::Terminal)
    ) {
        return false;
    }

    if let Some(buf) = ws.buffers.get_mut(&buf_id) {
        let delta = buf.insert(cursor, &filtered);
        // Cursor columns are byte offsets (see Pos docs); advance by the inserted
        // byte length, not the char count, or multi-byte input desyncs the cursor
        // from the buffer offset.
        let bytes = filtered.len();
        let cursor_event = ws.active_view_mut().map(|view| {
            view.cursor.col += bytes;
            view.col_memory = view.cursor.col;
            view.scroll_to_cursor(view.page_height.max(1));
            EditorEvent::CursorMoved { view_id: view.id, pos: view.cursor }
        });
        if let Some(event) = cursor_event {
            ws.emit(event);
        }
        ws.emit(EditorEvent::BufferChanged { id: buf_id, delta });
        return true;
    }
    false
}

fn keycode_to_char(key: aurea::KeyCode, shift: bool) -> Option<char> {
    use aurea::KeyCode::*;
    Some(match key {
        A => if shift { 'A' } else { 'a' }, B => if shift { 'B' } else { 'b' },
        C => if shift { 'C' } else { 'c' }, D => if shift { 'D' } else { 'd' },
        E => if shift { 'E' } else { 'e' }, F => if shift { 'F' } else { 'f' },
        G => if shift { 'G' } else { 'g' }, H => if shift { 'H' } else { 'h' },
        I => if shift { 'I' } else { 'i' }, J => if shift { 'J' } else { 'j' },
        K => if shift { 'K' } else { 'k' }, L => if shift { 'L' } else { 'l' },
        M => if shift { 'M' } else { 'm' }, N => if shift { 'N' } else { 'n' },
        O => if shift { 'O' } else { 'o' }, P => if shift { 'P' } else { 'p' },
        Q => if shift { 'Q' } else { 'q' }, R => if shift { 'R' } else { 'r' },
        S => if shift { 'S' } else { 's' }, T => if shift { 'T' } else { 't' },
        U => if shift { 'U' } else { 'u' }, V => if shift { 'V' } else { 'v' },
        W => if shift { 'W' } else { 'w' }, X => if shift { 'X' } else { 'x' },
        Y => if shift { 'Y' } else { 'y' }, Z => if shift { 'Z' } else { 'z' },
        Key0 => if shift { ')' } else { '0' }, Key1 => if shift { '!' } else { '1' },
        Key2 => if shift { '@' } else { '2' }, Key3 => if shift { '#' } else { '3' },
        Key4 => if shift { '$' } else { '4' }, Key5 => if shift { '%' } else { '5' },
        Key6 => if shift { '^' } else { '6' }, Key7 => if shift { '&' } else { '7' },
        Key8 => if shift { '*' } else { '8' }, Key9 => if shift { '(' } else { '9' },
        Space => ' ',
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Rendering constants — Catppuccin Mocha
// ---------------------------------------------------------------------------

const BG:           Color = Color::rgb(30,  30,  46);
const GUTTER_BG:    Color = Color::rgb(24,  24,  37);
const GUTTER_FG:    Color = Color::rgb(88,  91, 112);
const GUTTER_ACT:   Color = Color::rgb(205, 214, 244);
const STATUSBAR_BG: Color = Color::rgb(24,  24,  37);
const STATUSBAR_FG: Color = Color::rgb(166, 227, 161);
const STATUSBAR_DIM: Color = Color::rgb(137, 180, 250);
const STATUS_MODE_BG: Color = Color::rgb(49,  50,  68);
const BORDER:       Color = Color::rgb(49,  50,  68);
const CURSOR_BG:    Color = Color::rgba(245, 224, 220, 220);
const CURSOR_LINE:  Color = Color::rgba(49,  50,  68, 140);
const ACTIVE_PANE_BORDER: Color = Color::rgb(137, 180, 250);
const SCROLLBAR_THUMB: Color = Color::rgba(88, 91, 112, 180);

// Catppuccin Mocha syntax palette
fn token_color(kind: TokenKind) -> Color {
    match kind {
        TokenKind::Keyword        => Color::rgb(203, 166, 247), // mauve
        TokenKind::KeywordControl => Color::rgb(243, 139, 168), // red
        TokenKind::Type           => Color::rgb(137, 180, 250), // blue
        TokenKind::String         => Color::rgb(166, 227, 161), // green
        TokenKind::Comment        => Color::rgb(88,  91,  112), // overlay0
        TokenKind::Number         => Color::rgb(250, 179, 135), // peach
        TokenKind::Macro          => Color::rgb(137, 220, 235), // sky
        TokenKind::Attribute      => Color::rgb(245, 194, 231), // flamingo
        TokenKind::Lifetime       => Color::rgb(245, 194, 231), // flamingo
        TokenKind::Function       => Color::rgb(137, 180, 250), // blue
        TokenKind::Operator       => Color::rgb(137, 220, 235), // sky
        TokenKind::SectionHeader  => Color::rgb(203, 166, 247), // mauve
        _                         => Color::rgb(205, 214, 244), // text
    }
}

const GUTTER_MIN_W: f32 = 52.0;
const PAD:      f32 = 8.0;
const STATUS_H: f32 = 28.0;
const EDITOR_TOP_PAD: f32 = 10.0;
const SPLIT_GAP: f32 = 4.0;

fn editor_font(config: &Config) -> Font {
    Font::new(&config.editor.font, config.editor.font_size)
}

// ---------------------------------------------------------------------------
// draw_editor
// ---------------------------------------------------------------------------

fn draw_editor(ctx: &mut dyn DrawingContext, ws: &mut Workspace, config: &Config) -> AureaResult<()> {
    let width  = ctx.width()  as f32;
    let height = ctx.height() as f32;

    ctx.clear(BG)?;

    let font   = editor_font(config);
    let metrics = ctx.measure_text("M", &font).ok();
    let char_w = metrics.as_ref().map(|m| m.advance).unwrap_or(font.size * 0.6);
    let text_ascent = metrics.as_ref().map(|m| m.ascent).unwrap_or(font.size * 0.8);
    let text_descent = metrics.as_ref().map(|m| m.descent).unwrap_or(font.size * 0.2);

    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let metrics = TextMetrics { char_w, text_ascent, text_descent };

    if let Some(panes) = &ws.panes {
        let panes = panes.clone();
        draw_pane_tree(ctx, ws, config, &panes, editor_rect, &font, metrics)?;
    } else if let Some(view_id) = ws.active_view().map(|view| view.id) {
        draw_view(ctx, ws, config, view_id, editor_rect, &font, metrics)?;
    }

    draw_status_bar(ctx, width, height, &font, ws)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct TextMetrics {
    char_w: f32,
    text_ascent: f32,
    text_descent: f32,
}

fn draw_pane_tree(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    tree: &PaneTree,
    rect: Rect,
    font: &Font,
    metrics: TextMetrics,
) -> AureaResult<()> {
    match tree {
        PaneTree::Leaf { view_id } => draw_view(ctx, ws, config, *view_id, rect, font, metrics),
        PaneTree::Split { axis, ratio, first, second } => {
            let (first_rect, second_rect, divider) = split_rect(rect, *axis, *ratio);
            draw_pane_tree(ctx, ws, config, first, first_rect, font, metrics)?;
            draw_pane_tree(ctx, ws, config, second, second_rect, font, metrics)?;
            ctx.draw_rect(divider, &solid(BORDER))?;
            Ok(())
        }
    }
}

fn draw_view(
    ctx: &mut dyn DrawingContext,
    ws: &mut Workspace,
    config: &Config,
    view_id: ViewId,
    rect: Rect,
    font: &Font,
    metrics: TextMetrics,
) -> AureaResult<()> {
    let Some(buffer_id) = ws.views.get(&view_id).map(|view| view.buffer_id) else {
        return Ok(());
    };
    let Some(line_count) = ws.buffers.get(&buffer_id).map(|buf| buf.line_count()) else {
        return Ok(());
    };

    let is_active_pane = ws.active_view_id == Some(view_id);
    let line_h = font.size * config.editor.line_height;
    let content_top = rect.y + EDITOR_TOP_PAD;
    let content_h = (rect.height - EDITOR_TOP_PAD).max(0.0);
    let visible = ((content_h / line_h) as usize).max(1);

    if let Some(view) = ws.views.get_mut(&view_id) {
        view.page_height = visible;
        view.scroll_line = view.scroll_line.min(max_scroll_line(line_count, visible));
    }

    let Some(view) = ws.views.get(&view_id) else {
        return Ok(());
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return Ok(());
    };

    ctx.draw_rect(rect, &solid(BG))?;

    // Filetype for syntax
    let ft = match &buf.kind {
        BufferKind::File(p) => Filetype::from_path(&p.to_string_lossy()),
        _ => Filetype::Plain,
    };

    let scroll      = view.scroll_line;
    let visible     = visible + 1;
    let gutter_w    = gutter_width(line_count, metrics.char_w, config.editor.line_numbers);
    let text_x      = rect.x + gutter_w + PAD;

    // Gutter strip
    if gutter_w > 0.0 {
        ctx.draw_rect(Rect::new(rect.x, rect.y, gutter_w, rect.height), &solid(GUTTER_BG))?;
    }

    // Pre-scan: walk from line 0 to scroll to find block-comment state.
    // Acceptable for Phase 1 file sizes.
    let mut scan_state = ScanState::clean();
    for l in 0..scroll {
        if let Some(text) = buf.line(l) {
            let (_, ns) = scan_line(ft, &text, scan_state);
            scan_state = ns;
        }
    }

    for i in 0..visible {
        let line_idx = scroll + i;
        if line_idx >= line_count { break; }

        let line_top = content_top + i as f32 * line_h;
        if line_top >= content_top + content_h || line_top >= rect.y + rect.height { break; }

        let baseline = baseline_in_rect(line_top, line_h, metrics.text_ascent, metrics.text_descent);
        let is_cursor = line_idx == view.cursor.line;

        // Cursor-line highlight
        if is_cursor && is_active_pane {
            ctx.draw_rect(Rect::new(rect.x, line_top + 1.0, rect.width, line_h - 1.0), &solid(CURSOR_LINE))?;
        }

        // Gutter line number (absolute / relative / off per config)
        let gutter_label = match config.editor.line_numbers {
            LineNumbers::Off => None,
            LineNumbers::Absolute => Some(format!("{:>4}", line_idx + 1)),
            LineNumbers::Relative => {
                if is_cursor {
                    Some(format!("{:<4}", line_idx + 1))
                } else {
                    let dist = line_idx.abs_diff(view.cursor.line);
                    Some(format!("{:>4}", dist))
                }
            }
        };
        if let Some(num) = gutter_label {
            let ng = if is_cursor { GUTTER_ACT } else { GUTTER_FG };
            let num_x = (rect.x + gutter_w - PAD - num.len() as f32 * metrics.char_w).max(rect.x + 4.0);
            ctx.draw_text_with_font(&num, Point::new(num_x, baseline), &font, &solid(ng))?;
        }

        // Line text with syntax
        if let Some(line_text) = buf.line(line_idx) {
            let (spans, new_state) = scan_line(ft, &line_text, scan_state);
            scan_state = new_state;

            if spans.is_empty() || ft == Filetype::Plain {
                ctx.draw_text_with_font(
                    &line_text,
                    Point::new(text_x, baseline),
                    &font,
                    &solid(token_color(TokenKind::Default)),
                )?;
            } else {
                draw_highlighted(ctx, &line_text, &spans, text_x, baseline, metrics.char_w, &font)?;
            }
        }

        if is_cursor && is_active_pane {
            draw_cursor(
                ctx,
                text_x + view.cursor.col as f32 * metrics.char_w,
                line_top,
                line_h,
                metrics.char_w,
                config.editor.cursor_style,
            )?;
        }
    }

    // Gutter divider
    if gutter_w > 0.0 {
        ctx.draw_line(rect.x + gutter_w, rect.y, rect.x + gutter_w, rect.y + rect.height, &stroke(BORDER, 1.0))?;
    }

    // Scrollbar thumb (right edge), only when content overflows the viewport.
    let viewport_lines = (content_h / line_h).max(1.0);
    if (line_count as f32) > viewport_lines {
        let track_h = rect.height;
        let thumb_h = (track_h * viewport_lines / line_count as f32).clamp(24.0, track_h);
        let max_scroll = max_scroll_line(line_count, viewport_lines as usize);
        let t = if max_scroll > 0 {
            (scroll as f32 / max_scroll as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_y = rect.y + t * (track_h - thumb_h);
        let bar_x = rect.x + rect.width - 4.0;
        ctx.draw_rect(Rect::new(bar_x, thumb_y, 3.0, thumb_h), &solid(SCROLLBAR_THUMB))?;
    }

    if is_active_pane {
        ctx.draw_rect(Rect::new(rect.x, rect.y, rect.width, 2.0), &solid(ACTIVE_PANE_BORDER))?;
    }

    Ok(())
}

/// Draw a line with per-token colouring. Gaps between spans use Default colour.
fn draw_highlighted(
    ctx: &mut dyn DrawingContext,
    text: &str,
    spans: &[ozone_syntax::TokenSpan],
    x0: f32,
    y: f32,
    char_w: f32,
    font: &Font,
) -> AureaResult<()> {
    let bytes = text.as_bytes();
    let mut last = 0usize;

    for span in spans {
        // Gap before this span
        if span.start > last {
            let seg = &text[last..span.start];
            let sx = x0 + last as f32 * char_w;
            ctx.draw_text_with_font(seg, Point::new(sx, y), font, &solid(token_color(TokenKind::Default)))?;
        }

        let end = (span.start + span.len).min(bytes.len());
        let seg = &text[span.start..end];
        let sx = x0 + span.start as f32 * char_w;
        ctx.draw_text_with_font(seg, Point::new(sx, y), font, &solid(token_color(span.kind)))?;

        last = end;
    }

    // Trailing gap
    if last < text.len() {
        let seg = &text[last..];
        let sx = x0 + last as f32 * char_w;
        ctx.draw_text_with_font(seg, Point::new(sx, y), font, &solid(token_color(TokenKind::Default)))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// draw_status_bar
// ---------------------------------------------------------------------------

fn draw_status_bar(
    ctx: &mut dyn DrawingContext,
    width: f32,
    height: f32,
    font: &Font,
    ws: &Workspace,
) -> AureaResult<()> {
    let bar_top = height - STATUS_H;
    ctx.draw_rect(Rect::new(0.0, bar_top, width, STATUS_H), &solid(STATUSBAR_BG))?;
    ctx.draw_line(0.0, bar_top, width, bar_top, &stroke(BORDER, 1.0))?;

    let (mode, file_name, cursor_info, dirty, filetype, pane_info) = if let (Some(view), Some(buf)) = (
        ws.active_view(), ws.active_buffer(),
    ) {
        let file_name = match &buf.kind {
            BufferKind::File(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(),
            BufferKind::Scratch => "*scratch*".to_string(),
            BufferKind::Search => "*files*".to_string(),
            BufferKind::References => "*references*".to_string(),
            BufferKind::Terminal => "*terminal*".to_string(),
        };
        let cursor_info = format!("{}:{}", view.cursor.line + 1, view.cursor.col + 1);
        let dirty = if buf.is_dirty() { "*" } else { "" };
        let filetype = match &buf.kind {
            BufferKind::File(p) => filetype_label(Filetype::from_path(&p.to_string_lossy())),
            _ => filetype_label(Filetype::Plain),
        };
        let pane_info = pane_status(ws, view.id);
        ("EDIT", file_name, cursor_info, dirty.to_string(), filetype, pane_info)
    } else {
        ("", String::new(), String::new(), String::new(), "plain", String::new())
    };

    let ascent = ctx
        .measure_text("M", font)
        .map(|m| m.ascent)
        .unwrap_or(font.size * 0.8);
    let descent = ctx
        .measure_text("M", font)
        .map(|m| m.descent)
        .unwrap_or(font.size * 0.2);
    let baseline = baseline_in_rect(bar_top, STATUS_H, ascent, descent);

    let mode_text = format!(" {} ", mode);
    let mode_w = ctx
        .measure_text(&mode_text, font)
        .map(|m| m.advance)
        .unwrap_or(font.size * 4.0);
    ctx.draw_rect(Rect::new(8.0, bar_top + 4.0, mode_w + 8.0, STATUS_H - 8.0), &solid(STATUS_MODE_BG))?;
    ctx.draw_text_with_font(&mode_text, Point::new(12.0, baseline), font, &solid(STATUSBAR_FG))?;

    let left = format!("  {}{}    {}", file_name, dirty, cursor_info);
    ctx.draw_text_with_font(&left, Point::new(16.0 + mode_w, baseline), font, &solid(STATUSBAR_FG))?;

    let right = if pane_info.is_empty() {
        format!("UTF-8  {}", filetype)
    } else {
        format!("{}  UTF-8  {}", pane_info, filetype)
    };
    let right_w = ctx
        .measure_text(&right, font)
        .map(|m| m.advance)
        .unwrap_or(right.len() as f32 * font.size * 0.6);
    let right_x = (width - right_w - 12.0).max(16.0 + mode_w);
    ctx.draw_text_with_font(&right, Point::new(right_x, baseline), font, &solid(STATUSBAR_DIM))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn baseline_in_rect(top: f32, height: f32, ascent: f32, descent: f32) -> f32 {
    top + (height + ascent - descent) / 2.0
}

fn gutter_width(line_count: usize, char_w: f32, mode: LineNumbers) -> f32 {
    if mode == LineNumbers::Off {
        return 0.0;
    }
    let digits = line_count.max(1).to_string().len().max(2);
    GUTTER_MIN_W.max((digits as f32 + 2.0) * char_w + PAD)
}

fn split_rect(rect: Rect, axis: SplitAxis, ratio: f32) -> (Rect, Rect, Rect) {
    let ratio = ratio.clamp(0.1, 0.9);
    match axis {
        SplitAxis::Vertical => {
            let first_w = (rect.width * ratio - SPLIT_GAP / 2.0).max(0.0);
            let divider_x = rect.x + first_w;
            let second_x = divider_x + SPLIT_GAP;
            let second_w = (rect.x + rect.width - second_x).max(0.0);
            (
                Rect::new(rect.x, rect.y, first_w, rect.height),
                Rect::new(second_x, rect.y, second_w, rect.height),
                Rect::new(divider_x, rect.y, SPLIT_GAP, rect.height),
            )
        }
        SplitAxis::Horizontal => {
            let first_h = (rect.height * ratio - SPLIT_GAP / 2.0).max(0.0);
            let divider_y = rect.y + first_h;
            let second_y = divider_y + SPLIT_GAP;
            let second_h = (rect.y + rect.height - second_y).max(0.0);
            (
                Rect::new(rect.x, rect.y, rect.width, first_h),
                Rect::new(rect.x, second_y, rect.width, second_h),
                Rect::new(rect.x, divider_y, rect.width, SPLIT_GAP),
            )
        }
    }
}

fn draw_cursor(
    ctx: &mut dyn DrawingContext,
    x: f32,
    line_top: f32,
    line_h: f32,
    char_w: f32,
    style: CursorStyle,
) -> AureaResult<()> {
    match style {
        CursorStyle::Bar => {
            ctx.draw_rect(Rect::new(x, line_top + 1.0, 2.0, line_h - 1.0), &solid(CURSOR_BG))?;
        }
        CursorStyle::Block => {
            ctx.draw_rect(Rect::new(x, line_top + 2.0, char_w.max(6.0), line_h - 3.0), &solid(CURSOR_BG))?;
        }
        CursorStyle::Underline => {
            ctx.draw_rect(Rect::new(x, line_top + line_h - 3.0, char_w.max(6.0), 2.0), &solid(CURSOR_BG))?;
        }
    }
    Ok(())
}

fn filetype_label(filetype: Filetype) -> &'static str {
    match filetype {
        Filetype::Rust => "rust",
        Filetype::Toml => "toml",
        Filetype::Json => "json",
        Filetype::Markdown => "markdown",
        Filetype::Plain => "plain",
    }
}

fn pane_status(ws: &Workspace, active: ViewId) -> String {
    let Some(panes) = &ws.panes else {
        return String::new();
    };
    let leaves = panes.leaves();
    if leaves.len() <= 1 {
        return String::new();
    }
    let Some(idx) = leaves.iter().position(|id| *id == active) else {
        return String::new();
    };
    format!("pane {}/{}", idx + 1, leaves.len())
}

fn max_scroll_line(line_count: usize, page_height: usize) -> usize {
    line_count.saturating_sub(page_height.max(1))
}

fn solid(c: Color) -> Paint { Paint::new().color(c).style(PaintStyle::Fill) }
fn stroke(c: Color, w: f32) -> Paint { Paint::new().color(c).style(PaintStyle::Stroke).stroke_width(w) }
