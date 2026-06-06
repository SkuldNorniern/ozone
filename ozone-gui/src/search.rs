//! In-buffer search (Meta+F): incremental literal search with match highlighting.
//!
//! `SearchState` holds the query and the byte-offset matches in the active
//! buffer; the renderer ([`crate::draw_view`]) highlights them and this module
//! draws the top-right find bar. Matching itself is `ozone-editor`'s
//! `find_matches` (no regex).

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point, Rect};
use ozone_editor::Workspace;

use crate::theme::{PALETTE_BG, PALETTE_BORDER, PALETTE_DESC, PALETTE_FG, solid};
use crate::{baseline_in_rect, fill_round_rect};

pub(crate) struct SearchState {
    pub(crate) query: String,
    /// Byte offsets of matches in the active buffer.
    pub(crate) matches: Vec<usize>,
    pub(crate) current: usize,
    case_sensitive: bool,
}

impl SearchState {
    pub(crate) fn new(case_sensitive: bool) -> Self {
        Self { query: String::new(), matches: Vec::new(), current: 0, case_sensitive }
    }
    fn current_offset(&self) -> Option<usize> {
        self.matches.get(self.current).copied()
    }
    fn next(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + 1) % self.matches.len();
        }
    }
    fn prev(&mut self) {
        if !self.matches.is_empty() {
            self.current = (self.current + self.matches.len() - 1) % self.matches.len();
        }
    }
}

/// Recompute matches for the active buffer from the current query.
pub(crate) fn search_recompute(s: &mut SearchState, ws: &Workspace) {
    let text = ws.active_buffer().map(|b| b.text()).unwrap_or_default();
    s.matches = ozone_editor::find_matches(&text, &s.query, s.case_sensitive);
    if s.current >= s.matches.len() {
        s.current = 0;
    }
}

/// Point `current` at the first match at/after the cursor (wrapping).
pub(crate) fn search_select_from_cursor(s: &mut SearchState, ws: &Workspace) {
    let from = ws
        .active_view()
        .and_then(|v| ws.buffers.get(&v.buffer_id).map(|b| b.pos_to_offset(v.cursor)))
        .unwrap_or(0);
    if let Some(i) = ozone_editor::search::first_match_from(&s.matches, from) {
        s.current = i;
    }
}

/// Move the cursor to the current match and scroll it into view.
pub(crate) fn search_jump(s: &SearchState, ws: &mut Workspace) {
    let Some(off) = s.current_offset() else { return };
    let pos = ws.active_buffer().map(|b| b.offset_to_pos(off));
    if let (Some(pos), Some(view)) = (pos, ws.active_view_mut()) {
        view.cursor = pos;
        view.col_memory = pos.col;
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

/// Handle a key while search is active. Returns whether a redraw is needed.
pub(crate) fn handle_search_key(
    key: aurea::KeyCode,
    search: &mut Option<SearchState>,
    ws: &mut Workspace,
) -> bool {
    use aurea::KeyCode::*;
    let Some(s) = search.as_mut() else { return false };
    match key {
        Escape => {
            *search = None;
            true
        }
        Enter | Down => {
            s.next();
            search_jump(s, ws);
            true
        }
        Up => {
            s.prev();
            search_jump(s, ws);
            true
        }
        Backspace => {
            s.query.pop();
            search_recompute(s, ws);
            search_select_from_cursor(s, ws);
            search_jump(s, ws);
            true
        }
        _ => false,
    }
}

/// Top-right find bar: `find: <query>   (i/n)`.
pub(crate) fn draw_search_bar(
    ctx: &mut dyn DrawingContext,
    s: &SearchState,
    font: &Font,
    width: f32,
) -> AureaResult<()> {
    let line_h = (font.size * 1.7).max(18.0);
    let m = ctx.measure_text("M", font).ok();
    let ascent = m.as_ref().map(|x| x.ascent).unwrap_or(font.size * 0.8);
    let descent = m.as_ref().map(|x| x.descent).unwrap_or(font.size * 0.2);

    let count = if s.matches.is_empty() {
        if s.query.is_empty() { String::new() } else { "  (no matches)".to_string() }
    } else {
        format!("  ({}/{})", s.current + 1, s.matches.len())
    };
    let text = format!("find: {}{}", s.query, count);
    let text_w = ctx.measure_text(&text, font).map(|m| m.advance).unwrap_or(text.len() as f32 * font.size * 0.6);

    let pad = 10.0;
    let bw = (text_w + pad * 2.0 + 16.0).min(width - 24.0);
    let bx = width - bw - 12.0;
    let by = 10.0;
    fill_round_rect(ctx, Rect::new(bx - 1.0, by - 1.0, bw + 2.0, line_h + 2.0), 9.0, PALETTE_BORDER)?;
    fill_round_rect(ctx, Rect::new(bx, by, bw, line_h), 8.0, PALETTE_BG)?;

    let bl = baseline_in_rect(by, line_h, ascent, descent);
    let prompt_w = ctx.measure_text("find: ", font).map(|m| m.advance).unwrap_or(0.0);
    ctx.draw_text_with_font("find: ", Point::new(bx + pad, bl), font, &solid(PALETTE_DESC))?;
    let rest = format!("{}{}", s.query, count);
    ctx.draw_text_with_font(&rest, Point::new(bx + pad + prompt_w, bl), font, &solid(PALETTE_FG))?;
    Ok(())
}
