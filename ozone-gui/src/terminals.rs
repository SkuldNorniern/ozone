//! Terminal session bookkeeping and terminal-row rendering.

use std::collections::HashMap;

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::Config;
use ozone_editor::{PaneTree, Workspace};
use ozone_term::Terminal;

use crate::TermCells;
use crate::layout::{EDITOR_TOP_PAD, PAD, split_rect};
use crate::theme::{palette, solid, term_color};

/// Live terminal sessions plus the per-terminal caches the run loop keeps in
/// lockstep: the latest colour grid, the last PTY grid size pushed, and the last
/// output version rendered. Grouped so they are created and pruned together.
pub(crate) struct Terminals {
    pub sessions: HashMap<BufferId, Terminal>,
    pub failed: std::collections::HashSet<BufferId>,
    pub cells: TermCells,
    pub sizes: HashMap<BufferId, (u16, u16)>,
    pub versions: HashMap<BufferId, u64>,
}

impl Terminals {
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            failed: std::collections::HashSet::new(),
            cells: TermCells::new(),
            sizes: HashMap::new(),
            versions: HashMap::new(),
        }
    }
}

/// Collect the on-screen rect of every terminal-buffer leaf in a pane tree,
/// mirroring `draw_pane_tree`'s geometry so the PTY can be sized to its pane.
pub(crate) fn collect_term_rects(
    ws: &Workspace,
    tree: &PaneTree,
    rect: Rect,
    out: &mut Vec<(BufferId, Rect)>,
) {
    match tree {
        PaneTree::Leaf { view_id } => {
            if let Some(bid) = ws.views.get(view_id).map(|v| v.buffer_id)
                && matches!(
                    ws.buffers.get(&bid).map(|b| &b.kind),
                    Some(BufferKind::Terminal)
                )
            {
                out.push((bid, rect));
            }
        }
        PaneTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (fr, sr, _) = split_rect(rect, *axis, *ratio);
            collect_term_rects(ws, first, fr, out);
            collect_term_rects(ws, second, sr, out);
        }
    }
}

/// Convert a pane rect to a terminal cell grid `(cols, rows)`, matching the
/// renderer's text origin (left `PAD`, right scrollbar, `EDITOR_TOP_PAD`) and
/// its real measured cell width `char_w`.
pub(crate) fn rect_to_grid(rect: Rect, config: &Config, char_w: f32) -> (u16, u16) {
    let cw = char_w.max(1.0);
    let lh = (config.editor.font_size * config.editor.line_height).max(1.0);
    let usable_w = (rect.width - PAD - 6.0).max(0.0);
    let usable_h = (rect.height - EDITOR_TOP_PAD).max(0.0);
    let cols = (usable_w / cw).clamp(8.0, 1000.0) as u16;
    let rows = (usable_h / lh).clamp(2.0, 1000.0) as u16;
    (cols, rows)
}

/// Draw one row of terminal cells: per-cell background fills, then runs of
/// glyphs batched by identical foreground colour into single text draws.
/// Honours reverse-video by swapping fg/bg.
pub(crate) fn draw_term_row(
    ctx: &mut dyn DrawingContext,
    row: &[ozone_term::Cell],
    x0: f32,
    line_top: f32,
    baseline: f32,
    line_h: f32,
    char_w: f32,
    font: &Font,
) -> AureaResult<()> {
    use ozone_term::Color as TC;

    let resolve = |c: &ozone_term::Cell| -> (aurea::render::Color, Option<aurea::render::Color>) {
        if c.inverse {
            return (
                term_color(c.bg, palette().background),
                Some(term_color(c.fg, palette().foreground)),
            );
        }
        let bg = match c.bg {
            TC::Default => None,
            other => Some(term_color(other, palette().background)),
        };
        (term_color(c.fg, palette().foreground), bg)
    };

    for (i, cell) in row.iter().enumerate() {
        if let (_, Some(bg)) = resolve(cell) {
            let bx = x0 + i as f32 * char_w;
            ctx.draw_rect(
                Rect::new(bx, line_top + 1.0, char_w + 0.5, line_h - 1.0),
                &solid(bg),
            )?;
        }
    }

    let mut i = 0usize;
    while i < row.len() {
        let (fg, _) = resolve(&row[i]);
        let start = i;
        let mut text = String::new();
        while i < row.len() && resolve(&row[i]).0 == fg {
            text.push(row[i].ch);
            i += 1;
        }
        if text.trim_end().is_empty() {
            continue;
        }
        let sx = x0 + start as f32 * char_w;
        ctx.draw_text_with_font(&text, Point::new(sx, baseline), font, &solid(fg))?;
    }

    Ok(())
}
