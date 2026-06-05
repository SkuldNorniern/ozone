use std::os::raw::c_void;
use std::sync::{Arc, Mutex};

use aurea::elements::{Box as AureaBox, BoxOrientation, Container};
use aurea::render::{Canvas, Color, DrawingContext, Font, Paint, PaintStyle, Point, Rect, RendererBackend};
use aurea::{AureaResult, Element, Window, WindowEvent};
use ozone_editor::{CommandContext, CommandRegistry, Workspace};
use ozone_editor::commands::register_defaults;

// ---------------------------------------------------------------------------
// SendableCanvas: makes Canvas usable across threads (raw ptr is window-only)
// ---------------------------------------------------------------------------

struct SendableCanvas(Canvas);

// SAFETY: Aurea canvases are created and used on the main thread only.
// We never send the handle across threads — only the Arc<Mutex<>> is shared,
// and all Canvas calls happen on the same OS thread in our event loop.
unsafe impl Send for SendableCanvas {}
unsafe impl Sync for SendableCanvas {}

impl std::ops::Deref for SendableCanvas {
    type Target = Canvas;
    fn deref(&self) -> &Self::Target {
        &self.0
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

// SharedCanvas: an Element that holds the canvas behind an Arc so we can
// keep a live reference after handing the canvas to the window layout.
struct SharedCanvas(Arc<Mutex<SendableCanvas>>);

impl Element for SharedCanvas {
    fn handle(&self) -> *mut c_void {
        self.0.lock().unwrap().handle()
    }
    unsafe fn invalidate_platform(&self, rect: Option<aurea::render::Rect>) {
        let g = self.0.lock().unwrap();
        unsafe { Element::invalidate_platform(&*g, rect) }
    }
}

// ---------------------------------------------------------------------------
// Public API
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

        // Build canvas with the CPU rasterizer
        let raw_canvas = Canvas::new(W, H, RendererBackend::Cpu)?;
        let workspace_for_draw = self.workspace.clone();

        raw_canvas.set_draw_callback(move |ctx| {
            let ws = workspace_for_draw.lock().unwrap();
            draw_editor(ctx, &ws)
        })?;

        // Wrap in Arc so we can keep a reference after handing off to layout
        let canvas_arc = Arc::new(Mutex::new(SendableCanvas(raw_canvas)));
        let shared = SharedCanvas(canvas_arc.clone());

        let mut root = AureaBox::new(BoxOrientation::Vertical)?;
        root.add_weighted(shared, 1.0)?;
        window.set_content(root)?;

        // Trigger first draw
        canvas_arc.lock().unwrap().invalidate_all();

        // --------------- event loop ---------------
        loop {
            let events = window.poll_events();
            let mut should_close = false;
            let mut needs_redraw = false;

            for event in events {
                match event {
                    WindowEvent::CloseRequested => {
                        should_close = true;
                    }
                    WindowEvent::Resized { .. } => {
                        needs_redraw = true;
                    }
                    WindowEvent::KeyInput { key, pressed: true, modifiers } => {
                        let redrew = handle_key(
                            key,
                            modifiers,
                            &mut self.workspace.lock().unwrap(),
                            &self.commands,
                        );
                        if redrew {
                            needs_redraw = true;
                        }
                    }
                    WindowEvent::TextInput { text } => {
                        insert_text(&text, &mut self.workspace.lock().unwrap(), &self.commands);
                        needs_redraw = true;
                    }
                    _ => {}
                }
            }

            if should_close {
                break;
            }

            if needs_redraw {
                canvas_arc.lock().unwrap().invalidate_all();
            }

            unsafe { aurea::ffi::ng_platform_poll_events() };
            window.process_frames()?;

            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Key routing
// ---------------------------------------------------------------------------

fn handle_key(
    key: aurea::KeyCode,
    mods: aurea::Modifiers,
    ws: &mut Workspace,
    reg: &CommandRegistry,
) -> bool {
    use aurea::KeyCode::*;

    let cmd = if mods.ctrl && !mods.shift && !mods.alt {
        match key {
            S => Some("file.save"),
            Z => Some("edit.undo"),
            Y => Some("edit.redo"),
            Home => Some("cursor.file-start"),
            End  => Some("cursor.file-end"),
            _ => None,
        }
    } else if !mods.ctrl && !mods.alt {
        match key {
            Up       => Some("cursor.move-up"),
            Down     => Some("cursor.move-down"),
            Left     => Some("cursor.move-left"),
            Right    => Some("cursor.move-right"),
            Home     => Some("cursor.line-start"),
            End      => Some("cursor.line-end"),
            Backspace => Some("edit.delete-char-backward"),
            Delete   => Some("edit.delete-char-forward"),
            Enter    => Some("edit.insert-newline"),
            _ => None,
        }
    } else {
        None
    };

    if let Some(name) = cmd {
        if let Some(mut ctx) = CommandContext::new(ws) {
            reg.execute(name, &mut ctx);
            // Scroll to keep cursor visible (approximate 40 lines)
            if let Some(view) = ws.active_view_mut() {
                view.scroll_to_cursor(40);
            }
            return true;
        }
    }

    false
}

fn insert_text(text: &str, ws: &mut Workspace, _reg: &CommandRegistry) {
    // TextInput carries printable characters; ignore control characters
    if text.chars().any(|c| c.is_control()) {
        return;
    }
    let Some(view) = ws.active_view() else { return };
    let cursor = view.cursor;
    let buf_id = view.buffer_id;
    if let Some(buf) = ws.buffers.get_mut(&buf_id) {
        buf.insert(cursor, text);
        // Advance cursor by the number of chars inserted
        let char_count = text.chars().count();
        if let Some(view) = ws.active_view_mut() {
            view.cursor.col += char_count;
            view.col_memory = view.cursor.col;
            view.scroll_to_cursor(40);
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

// Catppuccin Mocha palette
const BG:          Color = Color::rgb(30,  30,  46);   // base
const GUTTER_BG:   Color = Color::rgb(24,  24,  37);   // mantle
const GUTTER_FG:   Color = Color::rgb(88,  91, 112);   // overlay0
const GUTTER_ACT:  Color = Color::rgb(205, 214, 244);  // text
const TEXT_FG:     Color = Color::rgb(205, 214, 244);  // text
const CURSOR_BG:   Color = Color::rgba(245, 224, 220, 220); // rosewater
const STATUSBAR_BG: Color = Color::rgb(24,  24,  37);
const STATUSBAR_FG: Color = Color::rgb(166, 227, 161); // green

const GUTTER_W:   f32 = 52.0;
const PAD:        f32 = 8.0;
const STATUS_H:   f32 = 26.0;

fn editor_font() -> Font {
    // "Consolas" is the default Windows monospace terminal font
    Font::new("Consolas", 14.0)
}

fn draw_editor(ctx: &mut dyn DrawingContext, ws: &Workspace) -> AureaResult<()> {
    let width  = ctx.width()  as f32;
    let height = ctx.height() as f32;

    // Background
    ctx.clear(BG)?;

    let font = editor_font();

    // Measure monospace character width
    let char_metrics = ctx.measure_text("M", &font)?;
    let char_w = char_metrics.advance;
    let line_h = font.size * 1.4;

    let content_top = 0.0f32;
    let content_h   = height - STATUS_H;
    // Exact count: one extra line so the last partial line is drawn; no +2
    // overshoot that would bleed into the status bar.
    let visible_lines = ((content_h / line_h) as usize).max(1) + 1;

    let Some(view) = ws.active_view() else {
        draw_status_bar(ctx, width, height, STATUS_H, &font, ws)?;
        return Ok(());
    };
    let Some(buf) = ws.buffers.get(&view.buffer_id) else {
        draw_status_bar(ctx, width, height, STATUS_H, &font, ws)?;
        return Ok(());
    };

    let line_count = buf.line_count();
    let scroll = view.scroll_line;

    // Gutter background
    ctx.draw_rect(Rect::new(0.0, content_top, GUTTER_W, content_h), &solid(GUTTER_BG))?;

    for i in 0..visible_lines {
        let line_idx = scroll + i;
        if line_idx >= line_count {
            break;
        }
        // Top of the line rect; skip if it would start inside the status bar.
        let line_top = content_top + i as f32 * line_h;
        if line_top >= content_h {
            break;
        }
        let y = line_top + line_h; // baseline (bottom of line rect)

        let is_cursor_line = line_idx == view.cursor.line;

        // Cursor-line highlight
        if is_cursor_line {
            ctx.draw_rect(
                Rect::new(0.0, y - line_h + 2.0, width, line_h - 1.0),
                &solid(Color::rgba(49, 50, 68, 120)),
            )?;
        }

        // Line number
        let num = format!("{:>4}", line_idx + 1);
        let gutter_color = if is_cursor_line { GUTTER_ACT } else { GUTTER_FG };
        ctx.draw_text_with_font(&num, Point::new(4.0, y), &font, &solid(gutter_color))?;

        // Line content
        if let Some(line_text) = buf.line(line_idx) {
            ctx.draw_text_with_font(
                &line_text,
                Point::new(GUTTER_W + PAD, y),
                &font,
                &solid(TEXT_FG),
            )?;
        }

        // Cursor
        if is_cursor_line {
            let cx = GUTTER_W + PAD + view.cursor.col as f32 * char_w;
            ctx.draw_rect(
                Rect::new(cx, y - line_h + 2.0, 2.0, line_h - 2.0),
                &solid(CURSOR_BG),
            )?;
        }
    }

    // Gutter divider
    ctx.draw_line(
        GUTTER_W, content_top, GUTTER_W, content_h,
        &Paint::new().color(Color::rgb(49, 50, 68)).style(PaintStyle::Stroke).stroke_width(1.0),
    )?;

    draw_status_bar(ctx, width, height, STATUS_H, &font, ws)?;

    Ok(())
}

fn draw_status_bar(
    ctx: &mut dyn DrawingContext,
    width: f32,
    height: f32,
    bar_h: f32,
    font: &Font,
    ws: &Workspace,
) -> AureaResult<()> {
    let y = height - bar_h;
    // Fill background
    ctx.draw_rect(Rect::new(0.0, y, width, bar_h), &solid(STATUSBAR_BG))?;
    // Top separator
    ctx.draw_line(
        0.0, y, width, y,
        &Paint::new().color(Color::rgb(49, 50, 68)).style(PaintStyle::Stroke).stroke_width(1.0),
    )?;

    let (mode, file_name, cursor_info, dirty) = if let (Some(view), Some(buf)) = (
        ws.active_view(),
        ws.active_buffer(),
    ) {
        let file_name = match &buf.kind {
            ozone_buffer::BufferKind::File(p) => p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string(),
            _ => "*scratch*".to_string(),
        };
        let cursor_info = format!("{}:{}", view.cursor.line + 1, view.cursor.col + 1);
        let dirty = if buf.is_dirty() { " ●" } else { "" };
        ("NORMAL", file_name, cursor_info, dirty.to_string())
    } else {
        ("", String::new(), String::new(), String::new())
    };

    // Position baseline so the glyph body is visually centred in the bar.
    // Approximation: baseline ≈ bar_top + (bar_h + font_size * 0.75) / 2
    let baseline = y + (bar_h + font.size * 0.75) / 2.0;

    let text = format!("  {}  {}{}    {}  UTF-8", mode, file_name, dirty, cursor_info);
    ctx.draw_text_with_font(
        &text,
        Point::new(4.0, baseline),
        font,
        &solid(STATUSBAR_FG),
    )?;

    Ok(())
}

fn solid(c: Color) -> Paint {
    Paint::new().color(c).style(PaintStyle::Fill)
}
