//! Status bar rendering: the strip at the bottom of the editor window.

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};
use ozone_buffer::{BufferId, BufferKind};
use ozone_editor::{ViewId, Workspace, buffer_language};
use taste::Language;

use crate::components::draw_pill;
use crate::input::ActiveMods;
use crate::layout::{STATUS_H, baseline_in_rect};
use crate::lsp::LspStatus;
use crate::theme::{palette, solid, stroke};

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_status_bar(
    ctx: &mut dyn DrawingContext,
    width: f32,
    height: f32,
    font: &Font,
    ws: &Workspace,
    mods: ActiveMods,
    lsp_status: LspStatus,
    search_progress: Option<(usize, usize)>,
) -> AureaResult<()> {
    let bar_top = height - STATUS_H;
    ctx.draw_rect(
        Rect::new(0.0, bar_top, width, STATUS_H),
        &solid(palette().statusbar_bg),
    )?;
    ctx.draw_line(0.0, bar_top, width, bar_top, &stroke(palette().border, 1.0))?;

    // Progress bar: a 3 px accent strip at the top of the status bar while a
    // workspace search is running. Filled portion = scanned / total.
    if let Some((scanned, total)) = search_progress {
        let frac = if total > 0 {
            (scanned as f32 / total as f32).min(1.0)
        } else {
            0.0
        };
        let filled = width * frac;
        // Full-width dim track so the bar is visible even at 0 %.
        ctx.draw_rect(
            Rect::new(0.0, bar_top, width, 3.0),
            &solid(palette().statusbar_dim),
        )?;
        if filled > 0.0 {
            ctx.draw_rect(
                Rect::new(0.0, bar_top, filled, 3.0),
                &solid(palette().picker_prompt),
            )?;
        }
    }

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
                BufferKind::File(_) => major_mode_label(buffer_language(buf)),
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
            (
                "",
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
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

    draw_buffer_dots(ctx, ws, width, bar_top, baseline, font)?;

    // LSP status chip: only when relevant (starting or failed).
    if let Some((label, color)) = lsp_chip(lsp_status) {
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
            color,
        )?;
        x -= 6.0;
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

// --- buffer dots -----------------------------------------------------------

/// Horizontal gap between buffer dots (also the click target width).
const DOT_SPACING: f32 = 16.0;
/// Most dots drawn before collapsing the remainder into a `+N` label.
const DOT_MAX: usize = 16;

/// The buffers represented by status-bar dots, in stable creation order.
/// Transient UI surfaces (pickers, file tree, references) are excluded; only
/// real content buffers count.
pub(crate) fn switchable_buffers(ws: &Workspace) -> Vec<BufferId> {
    let mut ids: Vec<BufferId> = ws
        .buffers
        .iter()
        .filter(|(_, b)| {
            matches!(
                b.kind,
                BufferKind::File(_)
                    | BufferKind::Scratch
                    | BufferKind::Image(_)
                    | BufferKind::Terminal
            )
        })
        .map(|(id, _)| *id)
        .collect();
    ids.sort_by_key(|id| id.raw());
    ids
}

/// Center x of each *shown* dot, centered horizontally in the bar. Deterministic
/// from `(count, width)` alone so the renderer and the click hit-test agree
/// without sharing geometry.
fn dot_centers(count: usize, width: f32) -> Vec<f32> {
    let shown = count.min(DOT_MAX);
    if shown == 0 {
        return Vec::new();
    }
    let total = (shown as f32 - 1.0) * DOT_SPACING;
    let start = (width - total) / 2.0;
    (0..shown).map(|i| start + i as f32 * DOT_SPACING).collect()
}

/// The buffer whose dot is under `(x, y)`, or `None`. Used by the mouse handler
/// (opt-in) to switch buffers on click. Hidden with 0–1 buffers.
pub(crate) fn buffer_dot_at(
    ws: &Workspace,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
) -> Option<BufferId> {
    let bar_top = height - STATUS_H;
    if y < bar_top {
        return None;
    }
    let ids = switchable_buffers(ws);
    if ids.len() <= 1 {
        return None;
    }
    let cy = bar_top + STATUS_H / 2.0;
    dot_centers(ids.len(), width)
        .into_iter()
        .zip(ids)
        .find(|(cx, _)| (x - cx).abs() <= DOT_SPACING / 2.0 && (y - cy).abs() <= STATUS_H / 2.0)
        .map(|(_, id)| id)
}

/// Draw the buffer dots: filled accent for the active buffer, a warn tint for
/// other dirty buffers, dim for the rest. Collapses to `+N` past `DOT_MAX`.
/// Hidden with 0–1 buffers (no value, just noise).
fn draw_buffer_dots(
    ctx: &mut dyn DrawingContext,
    ws: &Workspace,
    width: f32,
    bar_top: f32,
    baseline: f32,
    font: &Font,
) -> AureaResult<()> {
    let ids = switchable_buffers(ws);
    if ids.len() <= 1 {
        return Ok(());
    }
    let active = ws.active_view().map(|v| v.buffer_id);
    let centers = dot_centers(ids.len(), width);
    let cy = bar_top + STATUS_H / 2.0;
    let pal = palette();

    for (cx, id) in centers.iter().zip(&ids) {
        let is_active = Some(*id) == active;
        let dirty = ws.buffers.get(id).map(|b| b.is_dirty()).unwrap_or(false);
        let (radius, color) = if is_active {
            (4.0, pal.picker_prompt)
        } else if dirty {
            (3.0, pal.notify_warn)
        } else {
            (2.5, pal.statusbar_dim)
        };
        ctx.draw_circle(Point::new(*cx, cy), radius, &solid(color))?;
    }

    if ids.len() > DOT_MAX {
        let label = format!(" +{}", ids.len() - DOT_MAX);
        let x = centers.last().copied().unwrap_or(width / 2.0) + DOT_SPACING / 2.0;
        ctx.draw_text_with_font(
            &label,
            Point::new(x, baseline),
            font,
            &solid(pal.statusbar_dim),
        )?;
    }
    Ok(())
}

fn major_mode_label(lang: Option<Language>) -> &'static str {
    lang.map(|l| l.display_name()).unwrap_or("Text")
}

/// Label + color for the LSP status chip. Returns `None` when idle/ready (no
/// chip shown — silence is the happy path).
fn lsp_chip(status: LspStatus) -> Option<(&'static str, aurea::render::Color)> {
    let p = palette();
    match status {
        LspStatus::Starting => Some(("LSP…", p.statusbar_dim)),
        LspStatus::Failed => Some(("LSP ✗", p.notify_error)),
        LspStatus::Idle | LspStatus::Ready => None,
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
