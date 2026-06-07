//! Terminal session bookkeeping for the run loop: the live PTY sessions plus
//! the per-terminal caches kept in lockstep, and the pane→grid sizing math.

use std::collections::HashMap;

use aurea::render::Rect;
use ozone_buffer::{BufferId, BufferKind};
use ozone_config::Config;
use ozone_editor::{PaneTree, Workspace};
use ozone_term::Terminal;

use crate::TermCells;
use crate::layout::{EDITOR_TOP_PAD, PAD, split_rect};

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
