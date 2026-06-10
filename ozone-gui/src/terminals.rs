//! Terminal session bookkeeping and terminal-row rendering.

use std::collections::HashMap;

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::Config;
use ozone_editor::{PaneTree, Workspace};
use ozone_term::Terminal;

use crate::TermCells;
use crate::layout::{EDITOR_TOP_PAD, PAD, STATUS_H, split_rect};
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

    /// Per-frame reconcile: spawn a PTY for any new terminal buffer, prune caches
    /// for closed buffers, resize each PTY to its pane, and pull fresh output into
    /// the backing buffer (moving the active view's cursor to the PTY cursor).
    /// Returns whether anything changed enough to warrant a redraw.
    pub(crate) fn sync(
        &mut self,
        ws: &mut Workspace,
        config: &Config,
        win_w: u32,
        win_h: u32,
        measured_char_w: f32,
    ) -> bool {
        let mut needs_redraw = false;

        // Spawn a session for each terminal buffer that doesn't have one yet.
        let term_bufs: Vec<BufferId> = ws
            .buffers
            .iter()
            .filter(|(_, b)| matches!(b.kind, BufferKind::Terminal))
            .map(|(id, _)| *id)
            .collect();
        for id in &term_bufs {
            if self.sessions.contains_key(id) || self.failed.contains(id) {
                continue;
            }
            match Terminal::spawn() {
                Ok(term) => {
                    let cw = (config.editor.font_size * 0.6).max(1.0);
                    let lh = (config.editor.font_size * config.editor.line_height).max(1.0);
                    let cols = ((win_w as f32 - 60.0) / cw).clamp(20.0, 500.0) as u16;
                    let rows =
                        ((win_h as f32 - STATUS_H - EDITOR_TOP_PAD) / lh).clamp(5.0, 300.0) as u16;
                    term.resize(cols, rows);
                    self.sessions.insert(*id, term);
                }
                Err(e) => {
                    self.failed.insert(*id);
                    if let Some(buf) = ws.buffers.get_mut(id) {
                        buf.set_text(&format!("could not start terminal: {e}\n"));
                    }
                    needs_redraw = true;
                }
            }
        }

        // Prune caches for buffers that are gone.
        self.sessions.retain(|id, _| ws.buffers.contains_key(id));
        let live = &self.sessions;
        self.cells.retain(|id, _| live.contains_key(id));
        self.sizes.retain(|id, _| live.contains_key(id));

        // Resize each PTY to match its on-screen pane.
        let editor_rect = Rect::new(0.0, 0.0, win_w as f32, (win_h as f32 - STATUS_H).max(0.0));
        let mut want: Vec<(BufferId, Rect)> = Vec::new();
        if let Some(panes) = &ws.panes {
            collect_term_rects(ws, panes, editor_rect, &mut want);
        } else if let Some(bid) = ws.active_view().map(|v| v.buffer_id)
            && self.sessions.contains_key(&bid)
        {
            want.push((bid, editor_rect));
        }
        for (bid, rect) in want {
            let size = rect_to_grid(rect, config, measured_char_w);
            if self.sessions.contains_key(&bid) && self.sizes.get(&bid) != Some(&size) {
                self.sessions[&bid].resize(size.0, size.1);
                self.sizes.insert(bid, size);
            }
        }

        // Pull fresh output for any session whose version advanced.
        self.versions.retain(|id, _| self.sessions.contains_key(id));
        let active_term = crate::keys::active_terminal(ws);
        for (id, term) in self.sessions.iter() {
            let version = term.version();
            if self.versions.get(id) == Some(&version) {
                continue;
            }
            self.versions.insert(*id, version);
            self.cells.insert(*id, term.cell_snapshot());
            let text = term.output_snapshot();
            if let Some(buf) = ws.buffers.get_mut(id) {
                buf.set_text(&text);
            }
            if active_term == Some(*id) {
                let (cline, ccol) = term.cursor();
                let last = ws
                    .buffers
                    .get(id)
                    .map(|b| b.line_count().saturating_sub(1))
                    .unwrap_or(0);
                let line = cline.min(last);
                let col = ws
                    .buffers
                    .get(id)
                    .map(|b| b.line_len(line))
                    .unwrap_or(0)
                    .min(ccol);
                if let Some(view) = ws.active_view_mut() {
                    view.cursor = ozone_buffer::Pos::new(line, col);
                    view.scroll_to_cursor(view.page_height.max(1));
                }
            }
            needs_redraw = true;
        }

        needs_redraw
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
#[allow(clippy::too_many_arguments)]
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
