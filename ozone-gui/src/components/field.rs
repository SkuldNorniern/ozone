//! Single-line input field: an accent `prompt`, the typed `text`, and a caret.
//! The shared text row behind the minibuffer, the picker query, and the find
//! bar — each draws its own container, then calls this for the contents.

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};

use crate::components::style;
use crate::layout::baseline_in_rect;
use crate::theme::solid;

/// Draw `prompt` (accent) + `text` (fg) + a caret, left-aligned inside `rect`
/// and vertically centred. `rect` is the text area (already inset/padded by the
/// caller). Returns the x just past the caret, for trailing content like a
/// match count.
pub(crate) fn draw_field(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    prompt: &str,
    text: &str,
    font: &Font,
    ascent: f32,
    descent: f32,
) -> AureaResult<f32> {
    let s = style();
    let baseline = baseline_in_rect(rect.y, rect.height, ascent, descent);
    let measure = |ctx: &mut dyn DrawingContext, t: &str| {
        ctx.measure_text(t, font)
            .map(|m| m.advance)
            .unwrap_or(t.len() as f32 * font.size * 0.6)
    };

    let mut x = rect.x;
    if !prompt.is_empty() {
        let p = format!("{prompt} ");
        ctx.draw_text_with_font(&p, Point::new(x, baseline), font, &solid(s.accent))?;
        x += measure(ctx, &p);
    }
    ctx.draw_text_with_font(text, Point::new(x, baseline), font, &solid(s.fg))?;
    x += measure(ctx, text);

    let caret_x = x + 1.0;
    let caret_y = rect.y + rect.height * 0.2;
    let caret_h = rect.height * 0.6;
    ctx.draw_rect(Rect::new(caret_x, caret_y, 2.0, caret_h), &solid(s.fg))?;
    Ok(caret_x + 2.0)
}
