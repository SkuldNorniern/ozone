//! Panel primitives: rounded rects, the bordered floating panel, the modal
//! scrim, and edge/centre placement helpers. The base every popup draws on.

use aurea::AureaResult;
use aurea::render::{Color, DrawingContext, Rect};

use aurea::ui::{Component, Surface};

use crate::components::{style, with_ui};
use crate::theme::solid;

/// Paint a rounded surface with Aurea's shared canvas component.
pub(crate) fn fill_round_rect(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    r: f32,
    color: Color,
) -> AureaResult<()> {
    with_ui(ctx, |ui| Surface::new(color).radius(r).paint(ui, rect))
}

/// Dim the whole surface behind a modal popup.
pub(crate) fn draw_scrim(ctx: &mut dyn DrawingContext, width: f32, height: f32) -> AureaResult<()> {
    ctx.draw_rect(Rect::new(0.0, 0.0, width, height), &solid(style().scrim))
}

/// Draw a bordered rounded panel (1px border ring + background fill). Content is
/// drawn by the caller inside `rect`.
pub(crate) fn draw_panel(ctx: &mut dyn DrawingContext, rect: Rect, radius: f32) -> AureaResult<()> {
    let s = style();
    with_ui(ctx, |ui| {
        Surface::new(s.panel_bg)
            .radius(radius + 1.0)
            .border(1.0, s.panel_border)
            .paint(ui, rect.inset(-1.0))
    })
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
