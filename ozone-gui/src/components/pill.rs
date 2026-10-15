//! Ozone's badge geometry and colours, painted with Aurea components.

use crate::components::with_ui;
use aurea::AureaResult;
use aurea::render::{Color, DrawingContext, Font, Rect};
use aurea::ui::{Component, Label, Surface};

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_pill(
    ctx: &mut dyn DrawingContext,
    label: &str,
    x: f32,
    top: f32,
    height: f32,
    pad: f32,
    font: &Font,
    bg: Color,
    fg: Color,
) -> AureaResult<f32> {
    with_ui(ctx, |ui| {
        let label = Label::new(label, fg).font(font);
        let width = label.width(ui) + pad * 2.0;
        let rect = Rect::new(x, top, width, height);
        Surface::new(bg).radius(5.0).paint(ui, rect)?;
        label.paint(ui, rect.inset_xy(pad, 0.0))?;
        Ok(width)
    })
}
