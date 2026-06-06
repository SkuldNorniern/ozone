//! Which-key continuation popup.
//!
//! When a chord prefix is pending (e.g. after `C-k`), Ozone shows a small
//! bottom-anchored panel listing the keys that could come next and what each
//! does — the Emacs `which-key` / Spacemacs idea. The keymap supplies the raw
//! continuations ([`ozone_editor::Keymap::continuations`]); this module only
//! lays them out. The panel is purely informational; the pending prefix still
//! lives in the run loop's chord state.

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};

use crate::popup::{draw_panel, fill_round_rect};
use crate::theme::{PALETTE_DESC, PALETTE_FG, PALETTE_PROMPT, STATUS_MODE_BG, solid};
use crate::{STATUS_H, baseline_in_rect};

/// One which-key entry: the next stroke label and what it leads to (a command
/// display name, or `+prefix` for a deeper group).
pub(crate) struct WhichKeyEntry {
    pub key: String,
    pub desc: String,
    pub is_group: bool,
}

/// Draw the which-key panel anchored above the status bar. `prefix` is the
/// already-typed chord shown in the header (e.g. `"C-k"`).
pub(crate) fn draw_which_key(ctx: &mut dyn DrawingContext, prefix: &str, entries: &[WhichKeyEntry], font: &Font, width: f32, height: f32) -> AureaResult<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let metrics = ctx.measure_text("M", font).ok();
    let char_w = metrics.as_ref().map(|m| m.advance).unwrap_or(font.size * 0.6);
    let ascent = metrics.as_ref().map(|m| m.ascent).unwrap_or(font.size * 0.8);
    let descent = metrics.as_ref().map(|m| m.descent).unwrap_or(font.size * 0.2);
    let line_h = (font.size * 1.6).max(16.0);

    // Lay entries into columns sized to the widest "key  desc" cell.
    let cell_chars = entries.iter().map(|e| e.key.chars().count() + 2 + e.desc.chars().count()).max().unwrap_or(8);
    let cell_w = (cell_chars as f32 + 3.0) * char_w;
    let margin = 12.0;
    let pad = 10.0;
    let avail_w = (width - margin * 2.0 - pad * 2.0).max(cell_w);
    let cols = ((avail_w / cell_w).floor() as usize).max(1).min(entries.len());
    let rows = entries.len().div_ceil(cols);

    let panel_w = (cols as f32 * cell_w + pad * 2.0).min(width - margin * 2.0);
    let header_h = line_h;
    let panel_h = header_h + rows as f32 * line_h + pad;
    let panel_x = margin;
    let panel_y = (height - STATUS_H - panel_h - 6.0).max(6.0);
    let panel = Rect::new(panel_x, panel_y, panel_w, panel_h);

    draw_panel(ctx, panel, 8.0)?;

    // Header: the pending prefix.
    let head_base = baseline_in_rect(panel_y + 2.0, header_h, ascent, descent);
    ctx.draw_text_with_font(&format!("{prefix}-"), Point::new(panel_x + pad, head_base), font, &solid(PALETTE_PROMPT))?;

    // Entries, column-major so reading down a column is natural.
    let body_top = panel_y + header_h;
    for (idx, entry) in entries.iter().enumerate() {
        let col = idx / rows;
        let row = idx % rows;
        let cell_x = panel_x + pad + col as f32 * cell_w;
        let cell_top = body_top + row as f32 * line_h;
        let base = baseline_in_rect(cell_top, line_h, ascent, descent);

        // Key pill.
        let key_w = entry.key.chars().count() as f32 * char_w + 8.0;
        fill_round_rect(ctx, Rect::new(cell_x, cell_top + 2.0, key_w, line_h - 4.0), 4.0, STATUS_MODE_BG)?;
        ctx.draw_text_with_font(&entry.key, Point::new(cell_x + 4.0, base), font, &solid(PALETTE_PROMPT))?;

        let desc_color = if entry.is_group { PALETTE_DESC } else { PALETTE_FG };
        ctx.draw_text_with_font(&entry.desc, Point::new(cell_x + key_w + 6.0, base), font, &solid(desc_color))?;
    }

    Ok(())
}
