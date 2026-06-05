use std::os::raw::c_void;
use std::sync::{Arc, Mutex};

use aurea::render::{Canvas, Color, DrawingContext, Font, Paint, PaintStyle, Point, Rect, RendererBackend};
use aurea::{AureaResult, Element, Window, WindowEvent};
use ozone_buffer::BufferKind;
use ozone_editor::{CommandContext, CommandRegistry, Workspace};
use ozone_editor::commands::register_defaults;
use ozone_syntax::{Filetype, ScanState, TokenKind, scan_line};

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
}

impl OzoneGui {
    pub fn new(workspace: Workspace) -> Self {
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        Self {
            workspace: Arc::new(Mutex::new(workspace)),
            commands: Arc::new(reg),
        }
    }

    pub fn run(self) -> AureaResult<()> {
        const W: u32 = 1280;
        const H: u32 = 800;

        let mut window = Window::new("Ozone", W as i32, H as i32)?;

        let raw_canvas = Canvas::new(W, H, RendererBackend::Cpu)?;
        let workspace_for_draw = self.workspace.clone();

        raw_canvas.set_draw_callback(move |ctx| {
            let ws = workspace_for_draw.lock().unwrap();
            draw_editor(ctx, &ws)
        })?;

        let canvas_arc = Arc::new(Mutex::new(SendableCanvas(raw_canvas)));

        // Set canvas directly as window content — no Box wrapper.
        // Keeps the HWND hierarchy as canvas → NativeGuiWindow (one hop).
        // set_window_content resizes the canvas to fill the client area and
        // calls SetFocus(window) so keyboard input works immediately.
        window.set_content(SharedCanvas(canvas_arc.clone()))?;

        canvas_arc.lock().unwrap().invalidate_all();

        let mut last_title = String::new();

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
                        ) {
                            needs_redraw = true;
                        }
                    }

                    // Text input is the primary edit path. KeyInput handles commands
                    // and provides a simple ASCII fallback for backends without WM_CHAR.
                    WindowEvent::TextInput { text } => {
                        if insert_text_raw(&text, &mut self.workspace.lock().unwrap()) {
                            needs_redraw = true;
                        }
                    }

                    WindowEvent::MouseWheel { delta_y, .. } => {
                        let mut ws = self.workspace.lock().unwrap();
                        if let Some(view) = ws.active_view_mut() {
                            let lines = (delta_y.abs() * 3.0).round() as usize;
                            if lines > 0 {
                                if delta_y > 0.0 {
                                    view.scroll_line = view.scroll_line.saturating_sub(lines);
                                } else {
                                    view.scroll_line = view.scroll_line.saturating_add(lines);
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
                let ws = self.workspace.lock().unwrap();
                canvas.draw(|ctx| draw_editor(ctx, &ws))?;
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
                _ => format!("Ozone - {}scratch", dirty),
            }
        }
        None => "Ozone".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Key routing
// ---------------------------------------------------------------------------

fn handle_key(
    key: aurea::KeyCode,
    mods: aurea::Modifiers,
    allow_text_fallback: bool,
    ws: &mut Workspace,
    reg: &CommandRegistry,
) -> bool {
    use aurea::KeyCode::*;

    // Ctrl shortcuts (no Alt)
    if mods.ctrl && !mods.alt {
        let cmd = match key {
            S if !mods.shift => Some("file.save"),
            Z if !mods.shift => Some("edit.undo"),
            Y if !mods.shift => Some("edit.redo"),

            // Emacs-style movement for the default non-modal editor.
            A if !mods.shift => Some("cursor.line-start"),
            E if !mods.shift => Some("cursor.line-end"),
            B if !mods.shift => Some("cursor.move-left"),
            F if !mods.shift => Some("cursor.move-right"),
            P if !mods.shift => Some("cursor.move-up"),
            N if !mods.shift => Some("cursor.move-down"),

            Home  => Some("cursor.file-start"),
            End   => Some("cursor.file-end"),
            Left  => Some("cursor.word-backward"),
            Right => Some("cursor.word-forward"),
            _ => None,
        };
        if let Some(name) = cmd {
            run_cmd(name, ws, reg);
            return true;
        }
        return false;
    }

    // Navigation / editing (no Ctrl, no Alt)
    if !mods.ctrl && !mods.alt {
        let cmd = match key {
            Up        => Some("cursor.move-up"),
            Down      => Some("cursor.move-down"),
            Left      => Some("cursor.move-left"),
            Right     => Some("cursor.move-right"),
            Home      => Some("cursor.line-start"),
            End       => Some("cursor.line-end"),
            PageUp    => Some("view.page-up"),
            PageDown  => Some("view.page-down"),
            Backspace => Some("edit.delete-char-backward"),
            Delete    => Some("edit.delete-char-forward"),
            Enter     => Some("edit.insert-newline"),
            Tab       => None, // soft-tab insertion below
            _ => None,
        };

        if let Some(name) = cmd {
            run_cmd(name, ws, reg);
            return true;
        }

        if key == Tab {
            return insert_text_raw("    ", ws);
        }

        if allow_text_fallback && let Some(ch) = keycode_to_char(key, mods.shift) {
            let mut buf = [0u8; 4];
            return insert_text_raw(ch.encode_utf8(&mut buf), ws);
        }
    }

    false
}

fn run_cmd(name: &str, ws: &mut Workspace, reg: &CommandRegistry) {
    if let Some(mut ctx) = CommandContext::new(ws) {
        reg.execute(name, &mut ctx);
    }
    if let Some(view) = ws.active_view_mut() {
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

fn insert_text_raw(text: &str, ws: &mut Workspace) -> bool {
    let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
    if filtered.is_empty() { return false; }

    let Some(view) = ws.active_view() else { return false };
    let cursor = view.cursor;
    let buf_id = view.buffer_id;

    if let Some(buf) = ws.buffers.get_mut(&buf_id) {
        buf.insert(cursor, &filtered);
        let chars = filtered.chars().count();
        if let Some(view) = ws.active_view_mut() {
            view.cursor.col += chars;
            view.col_memory = view.cursor.col;
            view.scroll_to_cursor(view.page_height.max(1));
        }
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
const BORDER:       Color = Color::rgb(49,  50,  68);
const CURSOR_BG:    Color = Color::rgba(245, 224, 220, 220);
const CURSOR_LINE:  Color = Color::rgba(49,  50,  68, 140);

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

const GUTTER_W: f32 = 52.0;
const PAD:      f32 = 8.0;
const STATUS_H: f32 = 28.0;
const EDITOR_TOP_PAD: f32 = 10.0;

fn editor_font() -> Font { Font::new("Consolas", 14.0) }

// ---------------------------------------------------------------------------
// draw_editor
// ---------------------------------------------------------------------------

fn draw_editor(ctx: &mut dyn DrawingContext, ws: &Workspace) -> AureaResult<()> {
    let width  = ctx.width()  as f32;
    let height = ctx.height() as f32;

    ctx.clear(BG)?;

    let font   = editor_font();
    let line_h = font.size * 1.4;
    let content_top = EDITOR_TOP_PAD;
    let content_h = (height - content_top - STATUS_H).max(0.0);

    let metrics = ctx.measure_text("M", &font).ok();
    let char_w = metrics.as_ref().map(|m| m.advance).unwrap_or(font.size * 0.6);
    let text_ascent = metrics.as_ref().map(|m| m.ascent).unwrap_or(font.size * 0.8);
    let text_descent = metrics.as_ref().map(|m| m.descent).unwrap_or(font.size * 0.2);

    let Some(view) = ws.active_view() else {
        draw_status_bar(ctx, width, height, &font, ws)?;
        return Ok(());
    };
    let Some(buf) = ws.buffers.get(&view.buffer_id) else {
        draw_status_bar(ctx, width, height, &font, ws)?;
        return Ok(());
    };

    // Filetype for syntax
    let ft = match &buf.kind {
        BufferKind::File(p) => Filetype::from_path(&p.to_string_lossy()),
        _ => Filetype::Plain,
    };

    let line_count  = buf.line_count();
    let scroll      = view.scroll_line;
    let visible     = ((content_h / line_h) as usize).max(1) + 1;

    // Gutter strip
    ctx.draw_rect(Rect::new(0.0, 0.0, GUTTER_W, height), &solid(GUTTER_BG))?;

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
        if line_top >= content_top + content_h { break; }

        let baseline = baseline_in_rect(line_top, line_h, text_ascent, text_descent);
        let is_cursor = line_idx == view.cursor.line;

        // Cursor-line highlight
        if is_cursor {
            ctx.draw_rect(Rect::new(0.0, line_top + 1.0, width, line_h - 1.0), &solid(CURSOR_LINE))?;
        }

        // Gutter line number
        let num = format!("{:>4}", line_idx + 1);
        let ng  = if is_cursor { GUTTER_ACT } else { GUTTER_FG };
        ctx.draw_text_with_font(&num, Point::new(4.0, baseline), &font, &solid(ng))?;

        // Line text with syntax
        if let Some(line_text) = buf.line(line_idx) {
            let (spans, new_state) = scan_line(ft, &line_text, scan_state);
            scan_state = new_state;

            if spans.is_empty() || ft == Filetype::Plain {
                ctx.draw_text_with_font(
                    &line_text,
                    Point::new(GUTTER_W + PAD, baseline),
                    &font,
                    &solid(token_color(TokenKind::Default)),
                )?;
            } else {
                draw_highlighted(ctx, &line_text, &spans, GUTTER_W + PAD, baseline, char_w, &font)?;
            }
        }

        // Cursor bar
        if is_cursor {
            let cx = GUTTER_W + PAD + view.cursor.col as f32 * char_w;
            ctx.draw_rect(Rect::new(cx, line_top + 1.0, 2.0, line_h - 1.0), &solid(CURSOR_BG))?;
        }
    }

    // Gutter divider
    ctx.draw_line(GUTTER_W, 0.0, GUTTER_W, height, &stroke(BORDER, 1.0))?;

    draw_status_bar(ctx, width, height, &font, ws)?;
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

    let (mode, file_name, cursor_info, dirty) = if let (Some(view), Some(buf)) = (
        ws.active_view(), ws.active_buffer(),
    ) {
        let file_name = match &buf.kind {
            BufferKind::File(p) => p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(),
            _ => "*scratch*".to_string(),
        };
        let cursor_info = format!("{}:{}", view.cursor.line + 1, view.cursor.col + 1);
        let dirty = if buf.is_dirty() { "*" } else { "" };
        ("EDIT", file_name, cursor_info, dirty.to_string())
    } else {
        ("", String::new(), String::new(), String::new())
    };

    let text = format!("  {}  {}{}    {}  UTF-8", mode, file_name, dirty, cursor_info);
    let ascent = ctx
        .measure_text("M", font)
        .map(|m| m.ascent)
        .unwrap_or(font.size * 0.8);
    let descent = ctx
        .measure_text("M", font)
        .map(|m| m.descent)
        .unwrap_or(font.size * 0.2);
    let baseline = baseline_in_rect(bar_top, STATUS_H, ascent, descent);
    ctx.draw_text_with_font(&text, Point::new(4.0, baseline), font, &solid(STATUSBAR_FG))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn baseline_in_rect(top: f32, height: f32, ascent: f32, descent: f32) -> f32 {
    top + (height + ascent - descent) / 2.0
}

fn solid(c: Color) -> Paint { Paint::new().color(c).style(PaintStyle::Fill) }
fn stroke(c: Color, w: f32) -> Paint { Paint::new().color(c).style(PaintStyle::Stroke).stroke_width(w) }
