//! Selectable vertical list: rows of `primary` text with an optional
//! right-aligned `detail`, one row highlighted. The body of the fuzzy picker;
//! reusable by any future list overlay (completion, references, quickfix).

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};

use crate::components::panel::fill_round_rect;
use crate::components::style;
use crate::layout::baseline_in_rect;
use crate::theme::solid;

/// One row: `primary` on the left, optional `detail` right-aligned (dim).
pub(crate) struct ListRow<'a> {
    pub primary: &'a str,
    pub detail: &'a str,
}

/// Draw `rows` stacked from `top`, each `line_h` tall, within `[x, x+width]`,
/// inset by `pad`. The row at `selected` (index into `rows`) gets a highlight
/// chip. Detail is drawn only when it fits clear of the primary text.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_list(
    ctx: &mut dyn DrawingContext,
    x: f32,
    top: f32,
    width: f32,
    line_h: f32,
    pad: f32,
    rows: &[ListRow],
    selected: Option<usize>,
    font: &Font,
    ascent: f32,
    descent: f32,
) -> AureaResult<()> {
    let s = style();
    let measure = |ctx: &mut dyn DrawingContext, t: &str| {
        ctx.measure_text(t, font)
            .map(|m| m.advance)
            .unwrap_or(t.len() as f32 * font.size * 0.6)
    };

    for (i, row) in rows.iter().enumerate() {
        let y = top + i as f32 * line_h;
        if selected == Some(i) {
            fill_round_rect(
                ctx,
                Rect::new(x + pad, y, width - 2.0 * pad, line_h),
                6.0,
                s.selection,
            )?;
        }
        let bl = baseline_in_rect(y, line_h, ascent, descent);
        ctx.draw_text_with_font(row.primary, Point::new(x + pad + 8.0, bl), font, &solid(s.fg))?;

        if !row.detail.is_empty() {
            let dw = measure(ctx, row.detail);
            let name_w = measure(ctx, row.primary);
            let dx = x + width - pad - 8.0 - dw;
            if dx > x + pad + 8.0 + name_w + 16.0 {
                ctx.draw_text_with_font(row.detail, Point::new(dx, bl), font, &solid(s.dim))?;
            }
        }
    }
    Ok(())
}
