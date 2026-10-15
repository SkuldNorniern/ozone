//! Adapts picker/completion rows to Aurea's shared row painter.

use crate::components::{style, with_ui};
use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Rect};
use aurea::ui::{Component, DetailRow};

pub(crate) struct ListRow<'a> {
    pub primary: &'a str,
    pub detail: &'a str,
}

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
) -> AureaResult<()> {
    let s = style();
    with_ui(ctx, |ui| {
        for (i, row) in rows.iter().enumerate() {
            let mut item = DetailRow::new(row.primary, row.detail, s.fg, s.dim).font(font);
            if selected == Some(i) {
                item = item.selected(s.selection);
            }
            item.paint(
                ui,
                Rect::new(
                    x + pad,
                    top + i as f32 * line_h,
                    (width - 2.0 * pad).max(0.0),
                    line_h,
                ),
            )?;
        }
        Ok(())
    })
}
