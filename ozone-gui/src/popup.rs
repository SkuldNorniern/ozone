//! Reusable floating-panel (popup / overlay) primitives — the shared base every
//! small popup draws on.
//!
//! Every overlay — the fuzzy picker, the find/replace bar, the modifier pills,
//! the [which-key panel](crate::whichkey), the [notification toasts](crate::notify),
//! and future completion / hover popups — is the same shape: a rounded, bordered
//! panel (optionally over a dimmed scrim), positioned either centred or anchored
//! to an edge. This module owns those primitives so the drawing is defined once
//! and each overlay just lays out its content inside a [`Rect`]. Stateful popups
//! that own a list + lifetime (like notifications) keep their controller in their
//! own module and call back here only to draw.

use aurea::AureaResult;
use aurea::render::{Color, DrawingContext, Point, Rect};

use crate::theme::{palette, solid};

/// Fill a rounded rectangle using a cross of rects plus four corner circles.
/// The shared building block for every panel and pill.
pub(crate) fn fill_round_rect(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    r: f32,
    color: Color,
) -> AureaResult<()> {
    let r = r.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    if r <= 0.5 {
        return ctx.draw_rect(rect, &solid(color));
    }
    ctx.draw_rect(
        Rect::new(rect.x, rect.y + r, rect.width, rect.height - 2.0 * r),
        &solid(color),
    )?;
    ctx.draw_rect(
        Rect::new(rect.x + r, rect.y, rect.width - 2.0 * r, rect.height),
        &solid(color),
    )?;
    ctx.draw_circle(Point::new(rect.x + r, rect.y + r), r, &solid(color))?;
    ctx.draw_circle(
        Point::new(rect.x + rect.width - r, rect.y + r),
        r,
        &solid(color),
    )?;
    ctx.draw_circle(
        Point::new(rect.x + r, rect.y + rect.height - r),
        r,
        &solid(color),
    )?;
    ctx.draw_circle(
        Point::new(rect.x + rect.width - r, rect.y + rect.height - r),
        r,
        &solid(color),
    )?;
    Ok(())
}

/// Dim the whole surface behind a modal popup.
pub(crate) fn draw_scrim(ctx: &mut dyn DrawingContext, width: f32, height: f32) -> AureaResult<()> {
    ctx.draw_rect(Rect::new(0.0, 0.0, width, height), &solid(palette().scrim))
}

/// Draw a bordered rounded panel (1px border ring + background fill). Content is
/// drawn by the caller inside `rect`.
pub(crate) fn draw_panel(ctx: &mut dyn DrawingContext, rect: Rect, radius: f32) -> AureaResult<()> {
    fill_round_rect(
        ctx,
        Rect::new(
            rect.x - 1.0,
            rect.y - 1.0,
            rect.width + 2.0,
            rect.height + 2.0,
        ),
        radius + 1.0,
        palette().picker_border,
    )?;
    fill_round_rect(ctx, rect, radius, palette().picker_bg)?;
    Ok(())
}

/// A `w`×`h` panel centred within `outer_w`×`outer_h`, with its top clamped to
/// at least `min_top` so it never runs off the top edge.
pub(crate) fn centered_rect(outer_w: f32, outer_h: f32, w: f32, h: f32, min_top: f32) -> Rect {
    let x = ((outer_w - w) / 2.0).max(0.0);
    let y = ((outer_h - h) / 2.0).max(min_top);
    Rect::new(x, y, w, h)
}

/// A `w`×`h` panel anchored to the top-right corner with `margin` insets.
pub(crate) fn top_right_rect(outer_w: f32, w: f32, h: f32, margin: f32) -> Rect {
    Rect::new((outer_w - w - margin).max(margin), margin, w, h)
}
