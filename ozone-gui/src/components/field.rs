//! Adapts Ozone's modal input state to Aurea's prompt-line painter.

use crate::components::{style, with_ui};
use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Rect};
use aurea::ui::PromptLine;

pub(crate) fn draw_field(
    ctx: &mut dyn DrawingContext,
    rect: Rect,
    prompt: &str,
    text: &str,
    font: &Font,
) -> AureaResult<f32> {
    let s = style();
    with_ui(ctx, |ui| {
        PromptLine::new(prompt, text, s.fg, s.accent)
            .font(font)
            .paint_end(ui, rect)
    })
}
