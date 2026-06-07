//! Pill / badge: a rounded chip wrapping a short label. Used by the status-bar
//! modifier indicator and the which-key key hints — same shape, one helper.

use aurea::AureaResult;
use aurea::render::{Color, DrawingContext, Font, Point, Rect};

use crate::components::panel::fill_round_rect;
use crate::theme::solid;

const PILL_RADIUS: f32 = 5.0;

/// Draw a rounded chip filled `bg` containing `label` in `fg`. The chip's left
/// edge sits at `x`, spans `height` from `top`, and the label is padded `pad`
/// from the left edge and drawn on `baseline`. Returns the chip's total width
/// so callers can lay pills left-to-right (or pre-measure for right alignment).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_pill(
    ctx: &mut dyn DrawingContext,
    label: &str,
    x: f32,
    top: f32,
    height: f32,
    baseline: f32,
    pad: f32,
    font: &Font,
    bg: Color,
    fg: Color,
) -> AureaResult<f32> {
    let text_w = ctx
        .measure_text(label, font)
        .map(|m| m.advance)
        .unwrap_or(label.len() as f32 * font.size * 0.6);
    let width = text_w + pad * 2.0;
    fill_round_rect(ctx, Rect::new(x, top, width, height), PILL_RADIUS, bg)?;
    ctx.draw_text_with_font(label, Point::new(x + pad, baseline), font, &solid(fg))?;
    Ok(width)
}
