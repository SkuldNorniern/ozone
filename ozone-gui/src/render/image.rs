use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Image, Rect};

use crate::theme::{palette, solid};
use super::TextMetrics;

/// Draw an image centered in `rect`, scaled to fit while preserving aspect
/// ratio (never upscaling past 1:1). Shows a label if the image failed to load.
pub(super) fn draw_image_pane(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    image: Option<&Image>,
    font: &Font,
    metrics: TextMetrics,
) -> AureaResult<()> {
    let Some(img) = image else {
        let msg = "cannot display image";
        let w = ctx
            .measure_text(msg, font)
            .map(|m| m.advance)
            .unwrap_or(msg.len() as f32 * metrics.char_w);
        let bl = rect.y + rect.height / 2.0;
        ctx.draw_text_with_font(
            msg,
            aurea::render::Point::new(rect.x + (rect.width - w) / 2.0, bl),
            font,
            &solid(palette().picker_detail),
        )?;
        return Ok(());
    };
    if img.width == 0 || img.height == 0 {
        return Ok(());
    }

    let pad = 12.0;
    let avail_w = (rect.width - pad * 2.0).max(1.0);
    let avail_h = (rect.height - pad * 2.0).max(1.0);
    let iw = img.width as f32;
    let ih = img.height as f32;
    let scale = (avail_w / iw).min(avail_h / ih).min(1.0);
    let dw = iw * scale;
    let dh = ih * scale;
    let dx = rect.x + (rect.width - dw) / 2.0;
    let dy = rect.y + (rect.height - dh) / 2.0;
    ctx.draw_image_rect(img, Rect::new(dx, dy, dw, dh))?;

    let label = format!("{}×{}", img.width, img.height);
    let lw = ctx
        .measure_text(&label, font)
        .map(|m| m.advance)
        .unwrap_or(0.0);
    let ly = (rect.y + rect.height - 6.0).min(rect.y + rect.height);
    ctx.draw_text_with_font(
        &label,
        aurea::render::Point::new(rect.x + (rect.width - lw) / 2.0, ly),
        font,
        &solid(palette().picker_detail),
    )?;
    Ok(())
}
