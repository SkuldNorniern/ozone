//! Minibuffer: a one-line text prompt (Emacs minibuffer / Neovim `vim.ui.input`).
//!
//! Opened by a [`ozone_editor::UiIntent::Input`]; on `Enter` the typed text is
//! handed to a named command as its argument. Generic, so any command or plugin
//! can take free-form input without a bespoke widget.

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};

use crate::baseline_in_rect;
use crate::popup::draw_panel;
use crate::theme::{PALETTE_FG, PALETTE_PROMPT, solid};

pub(crate) struct Minibuffer {
    pub(crate) prompt: String,
    pub(crate) input: String,
    /// Command to run with `input` as its argument on submit.
    pub(crate) command: String,
}

impl Minibuffer {
    pub(crate) fn new(prompt: impl Into<String>, command: impl Into<String>) -> Self {
        Self { prompt: prompt.into(), input: String::new(), command: command.into() }
    }
}

/// Bottom-anchored prompt bar: `<prompt> <input>|`.
pub(crate) fn draw_minibuffer(
    ctx: &mut dyn DrawingContext,
    mb: &Minibuffer,
    font: &Font,
    width: f32,
    height: f32,
    status_h: f32,
) -> AureaResult<()> {
    let line_h = (font.size * 1.7).max(18.0);
    let m = ctx.measure_text("M", font).ok();
    let ascent = m.as_ref().map(|x| x.ascent).unwrap_or(font.size * 0.8);
    let descent = m.as_ref().map(|x| x.descent).unwrap_or(font.size * 0.2);
    let measure = |ctx: &mut dyn DrawingContext, t: &str| {
        ctx.measure_text(t, font).map(|m| m.advance).unwrap_or(t.len() as f32 * font.size * 0.6)
    };

    let pad = 10.0;
    let bh = line_h + 8.0;
    // Sit just above the status bar, full width minus a small inset.
    let by = (height - status_h - bh - 6.0).max(0.0);
    let panel = Rect::new(8.0, by, width - 16.0, bh);
    draw_panel(ctx, panel, 8.0)?;

    let bl = baseline_in_rect(by + 4.0, line_h, ascent, descent);
    let prompt = format!("{} ", mb.prompt);
    let prompt_w = measure(ctx, &prompt);
    ctx.draw_text_with_font(&prompt, Point::new(panel.x + pad, bl), font, &solid(PALETTE_PROMPT))?;
    let input_x = panel.x + pad + prompt_w;
    ctx.draw_text_with_font(&mb.input, Point::new(input_x, bl), font, &solid(PALETTE_FG))?;
    let caret_x = input_x + measure(ctx, &mb.input) + 1.0;
    ctx.draw_rect(Rect::new(caret_x, by + 6.0, 2.0, line_h - 6.0), &solid(PALETTE_FG))?;
    Ok(())
}
