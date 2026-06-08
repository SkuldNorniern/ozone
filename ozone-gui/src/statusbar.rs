//! Status bar rendering: the strip at the bottom of the editor window.

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};
use ozone_buffer::BufferKind;
use ozone_editor::{ViewId, Workspace};
use ozone_syntax::Filetype;

use crate::components::draw_pill;
use crate::input::ActiveMods;
use crate::layout::{STATUS_H, baseline_in_rect};
use crate::theme::{palette, solid, stroke};

pub(crate) fn draw_status_bar(
    ctx: &mut dyn DrawingContext,
    width: f32,
    height: f32,
    font: &Font,
    ws: &Workspace,
    mods: ActiveMods,
) -> AureaResult<()> {
    let bar_top = height - STATUS_H;
    ctx.draw_rect(
        Rect::new(0.0, bar_top, width, STATUS_H),
        &solid(palette().statusbar_bg),
    )?;
    ctx.draw_line(0.0, bar_top, width, bar_top, &stroke(palette().border, 1.0))?;

    let (mode, file_name, cursor_info, dirty, pane_info) =
        if let (Some(view), Some(buf)) = (ws.active_view(), ws.active_buffer()) {
            let file_name = match &buf.kind {
                BufferKind::File(p) | BufferKind::Image(p) => p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string(),
                BufferKind::Scratch => "*scratch*".to_string(),
                BufferKind::Search => "*files*".to_string(),
                BufferKind::References => "*references*".to_string(),
                BufferKind::FileTree => "*tree*".to_string(),
                BufferKind::Terminal => "*terminal*".to_string(),
            };
            let cursor_info = format!("{}:{}", view.cursor.line + 1, view.cursor.col + 1);
            let dirty = if buf.is_dirty() { "*" } else { "" };
            let mode = match &buf.kind {
                BufferKind::File(p) => major_mode_label(Filetype::from_path(&p.to_string_lossy())),
                BufferKind::Search => "Files",
                BufferKind::References => "Refs",
                BufferKind::FileTree => "Tree",
                BufferKind::Terminal => "Term",
                BufferKind::Image(_) => "Image",
                BufferKind::Scratch => "Text",
            };
            let pane_info = pane_status(ws, view.id);
            (mode, file_name, cursor_info, dirty.to_string(), pane_info)
        } else {
            ("", String::new(), String::new(), String::new(), String::new())
        };

    let em = ctx.measure_text("M", font).ok();
    let ascent = em.as_ref().map(|m| m.ascent).unwrap_or(font.size * 0.8);
    let descent = em.as_ref().map(|m| m.descent).unwrap_or(font.size * 0.2);
    let baseline = baseline_in_rect(bar_top, STATUS_H, ascent, descent);

    let mode_text = format!(" {} ", mode);
    let mode_w = ctx
        .measure_text(&mode_text, font)
        .map(|m| m.advance)
        .unwrap_or(font.size * 4.0);
    ctx.draw_rect(
        Rect::new(8.0, bar_top + 4.0, mode_w + 8.0, STATUS_H - 8.0),
        &solid(palette().status_mode_bg),
    )?;
    ctx.draw_text_with_font(
        &mode_text,
        Point::new(12.0, baseline),
        font,
        &solid(palette().statusbar_fg),
    )?;

    let left = format!("  {}{}    {}", file_name, dirty, cursor_info);
    ctx.draw_text_with_font(
        &left,
        Point::new(16.0 + mode_w, baseline),
        font,
        &solid(palette().statusbar_fg),
    )?;

    let mut x = width - 12.0;
    if mods.any() {
        let labels = [
            ("Shift", mods.shift),
            ("Super", mods.super_),
            ("Meta", mods.meta),
            ("Ctrl", mods.control),
        ];
        for (label, active) in labels {
            if !active {
                continue;
            }
            let chip_w = ctx
                .measure_text(label, font)
                .map(|m| m.advance)
                .unwrap_or(label.len() as f32 * font.size * 0.6)
                + 12.0;
            x -= chip_w;
            draw_pill(
                ctx,
                label,
                x,
                bar_top + 4.0,
                STATUS_H - 8.0,
                baseline,
                6.0,
                font,
                palette().status_mode_bg,
                palette().picker_prompt,
            )?;
            x -= 6.0;
        }
    }

    let right = if pane_info.is_empty() {
        "UTF-8".to_string()
    } else {
        format!("{}  UTF-8", pane_info)
    };
    let right_w = ctx
        .measure_text(&right, font)
        .map(|m| m.advance)
        .unwrap_or(right.len() as f32 * font.size * 0.6);
    let right_x = (x - right_w - 12.0).max(16.0 + mode_w);
    ctx.draw_text_with_font(
        &right,
        Point::new(right_x, baseline),
        font,
        &solid(palette().statusbar_dim),
    )?;

    Ok(())
}

fn major_mode_label(filetype: Filetype) -> &'static str {
    match filetype {
        Filetype::Rust => "Rust",
        Filetype::Toml => "TOML",
        Filetype::Json => "JSON",
        Filetype::Markdown => "Markdown",
        Filetype::Plain => "Text",
    }
}

fn pane_status(ws: &Workspace, active: ViewId) -> String {
    let Some(panes) = &ws.panes else {
        return String::new();
    };
    let leaves = panes.leaves();
    if leaves.len() <= 1 {
        return String::new();
    }
    let Some(idx) = leaves.iter().position(|id| *id == active) else {
        return String::new();
    };
    format!("pane {}/{}", idx + 1, leaves.len())
}
