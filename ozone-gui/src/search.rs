//! In-buffer search (Meta+F) and replace (Ctrl+H): incremental literal search
//! with match highlighting, plus optional replace of the current / all matches.
//!
//! `SearchState` holds the query and the byte-offset matches in the active
//! buffer; the renderer ([`crate::draw_view`]) highlights them and this module
//! draws the top-right find/replace bar. Matching is `ozone-editor`'s
//! `find_matches` (literal, no regex).

use aurea::AureaResult;
use aurea::render::{DrawingContext, Font, Point};
use ozone_buffer::BufferId;
use ozone_editor::{DecorationKind, EditorEvent, HlRole, NamespaceId, Workspace};

use crate::baseline_in_rect;
use crate::components::{draw_panel, top_right_rect};
use crate::theme::{palette, solid};

pub(crate) struct SearchState {
    pub(crate) query: String,
    /// Byte offsets of matches in the active buffer.
    pub(crate) matches: Vec<usize>,
    pub(crate) current: usize,
    case_sensitive: bool,
    /// Replacement text when in replace mode; `None` = find-only.
    pub(crate) replace: Option<String>,
    /// In replace mode, whether typed text edits the replacement (vs the query).
    pub(crate) focus_replace: bool,
    namespace: NamespaceId,
    buffer_id: Option<BufferId>,
}

impl SearchState {
    pub(crate) fn new(case_sensitive: bool, namespace: NamespaceId) -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
            case_sensitive,
            replace: None,
            focus_replace: false,
            namespace,
            buffer_id: None,
        }
    }

    /// Turn on replace mode (keeps the existing query). Typing stays on the
    /// query field; Tab switches to the replacement (VS Code-style).
    pub(crate) fn enable_replace(&mut self) {
        if self.replace.is_none() {
            self.replace = Some(String::new());
        }
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
pub(crate) fn search_recompute(s: &mut SearchState, ws: &mut Workspace) {
    let Some((buffer_id, text)) = ws.active_buffer().map(|b| (b.id, b.text())) else {
        close_search(s, ws);
        s.buffer_id = None;
        s.matches.clear();
        s.current = 0;
        return;
    };
    if let Some(previous) = s.buffer_id
        && previous != buffer_id
    {
        ws.decorations_mut()
            .clear_namespace_in(previous, s.namespace);
    }
    s.buffer_id = Some(buffer_id);
    s.matches = ozone_editor::find_matches(&text, &s.query, s.case_sensitive);
    if s.current >= s.matches.len() {
        s.current = 0;
    }
    sync_search_decorations(s, ws);
}

fn sync_search_decorations(s: &SearchState, ws: &mut Workspace) {
    let Some(buffer_id) = s.buffer_id else {
        return;
    };
    ws.decorations_mut()
        .clear_namespace_in(buffer_id, s.namespace);
    let width = s.query.len();
    if width == 0 {
        return;
    }
    for (index, &start) in s.matches.iter().enumerate() {
        let role = if index == s.current {
            HlRole::SearchCurrent
        } else {
            HlRole::Search
        };
        ws.decorations_mut().add(
            buffer_id,
            s.namespace,
            start,
            start + width,
            DecorationKind::Highlight(role),
        );
    }
}

fn close_search(s: &SearchState, ws: &mut Workspace) {
    if let Some(buffer_id) = s.buffer_id {
        ws.decorations_mut()
            .clear_namespace_in(buffer_id, s.namespace);
    }
}

/// Point `current` at the first match at/after the cursor (wrapping).
pub(crate) fn search_select_from_cursor(s: &mut SearchState, ws: &Workspace) {
    let from = ws
        .active_view()
        .and_then(|v| {
            ws.buffers
                .get(&v.buffer_id)
                .map(|b| b.pos_to_offset(v.cursor))
        })
        .unwrap_or(0);
    if let Some(i) = ozone_editor::search::first_match_from(&s.matches, from) {
        s.current = i;
    }
}

/// Move the cursor to the current match and scroll it into view.
pub(crate) fn search_jump(s: &SearchState, ws: &mut Workspace) {
    let Some(off) = s.current_offset() else {
        return;
    };
    let pos = ws.active_buffer().map(|b| b.offset_to_pos(off));
    if let (Some(pos), Some(view)) = (pos, ws.active_view_mut()) {
        view.cursor = pos;
        view.col_memory = pos.col;
        view.scroll_to_cursor(view.page_height.max(1));
    }
}

/// Replace the `qbytes`-long match at byte offset `off` with `repl` in the
/// active buffer. (Literal matches keep the query's byte length.)
fn replace_at(ws: &mut Workspace, off: usize, qbytes: usize, repl: &str) {
    let Some(buffer_id) = ws.active_buffer().map(|buffer| buffer.id) else {
        return;
    };
    let mut deltas = Vec::new();
    if let Some(buf) = ws.buffers.get_mut(&buffer_id) {
        if qbytes > 0
            && let Some(delta) = buf.delete_at(off, qbytes)
        {
            deltas.push(delta);
        }
        if !repl.is_empty() {
            let pos = buf.offset_to_pos(off);
            deltas.push(buf.insert(pos, repl));
        }
    }
    for delta in deltas {
        ws.emit(EditorEvent::BufferChanged {
            id: buffer_id,
            delta,
        });
    }
}

/// Replace the current match, then recompute and move to the next.
pub(crate) fn search_replace_current(s: &mut SearchState, ws: &mut Workspace) {
    let Some(off) = s.current_offset() else {
        return;
    };
    let qbytes = s.query.len();
    if qbytes == 0 {
        return;
    }
    let repl = s.replace.clone().unwrap_or_default();
    replace_at(ws, off, qbytes, &repl);
    search_recompute(s, ws);
    if s.current >= s.matches.len() {
        s.current = 0;
    }
    search_jump(s, ws);
}

/// Replace every match in the buffer (from the end so offsets stay valid).
pub(crate) fn search_replace_all(s: &mut SearchState, ws: &mut Workspace) {
    let qbytes = s.query.len();
    if qbytes == 0 || s.matches.is_empty() {
        return;
    }
    let repl = s.replace.clone().unwrap_or_default();
    let offsets = s.matches.clone();
    for &off in offsets.iter().rev() {
        replace_at(ws, off, qbytes, &repl);
    }
    search_recompute(s, ws);
    s.current = 0;
    search_jump(s, ws);
}

/// Handle a key while search is active. Returns whether a redraw is needed.
pub(crate) fn handle_search_key(
    key: aurea::KeyCode,
    mods: aurea::Modifiers,
    search: &mut Option<SearchState>,
    ws: &mut Workspace,
) -> bool {
    use aurea::KeyCode::*;
    let Some(s) = search.as_mut() else {
        return false;
    };
    let in_replace = s.replace.is_some();
    match key {
        Escape => {
            close_search(s, ws);
            *search = None;
            true
        }
        // Ctrl+Enter (or Alt+Enter) replaces all; plain Enter in replace mode
        // replaces the current match; otherwise Enter just steps to the next.
        Enter if in_replace && (mods.ctrl || mods.alt) => {
            search_replace_all(s, ws);
            true
        }
        Enter if in_replace => {
            search_replace_current(s, ws);
            true
        }
        Enter | Down => {
            s.next();
            sync_search_decorations(s, ws);
            search_jump(s, ws);
            true
        }
        Up => {
            s.prev();
            sync_search_decorations(s, ws);
            search_jump(s, ws);
            true
        }
        // Tab switches the typed-text focus between query and replacement.
        Tab if in_replace => {
            s.focus_replace = !s.focus_replace;
            true
        }
        Backspace => {
            if in_replace && s.focus_replace {
                if let Some(r) = s.replace.as_mut() {
                    r.pop();
                }
            } else {
                s.query.pop();
                search_recompute(s, ws);
                search_select_from_cursor(s, ws);
                search_jump(s, ws);
            }
            true
        }
        _ => false,
    }
}

/// Append typed text to the focused input (query or replacement). Returns
/// whether anything changed (caller recomputes/redraws).
pub(crate) fn search_input_text(s: &mut SearchState, text: &str, ws: &mut Workspace) -> bool {
    let mut changed = false;
    if s.replace.is_some() && s.focus_replace {
        if let Some(r) = s.replace.as_mut() {
            for c in text.chars().filter(|c| !c.is_control()) {
                r.push(c);
                changed = true;
            }
        }
    } else {
        for c in text.chars().filter(|c| !c.is_control()) {
            s.query.push(c);
            changed = true;
        }
        if changed {
            search_recompute(s, ws);
            search_select_from_cursor(s, ws);
        }
    }
    changed
}

/// Top-right find bar: `find: <query>   (i/n)`, with a second `replace:` line
/// when in replace mode. The focused input is marked.
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
        if s.query.is_empty() {
            String::new()
        } else {
            "  (no matches)".to_string()
        }
    } else {
        format!("  ({}/{})", s.current + 1, s.matches.len())
    };
    let find_text = format!("find: {}{}", s.query, count);
    let replace_text = s.replace.as_ref().map(|r| format!("replace: {r}"));

    let measure = |ctx: &mut dyn DrawingContext, t: &str| {
        ctx.measure_text(t, font)
            .map(|m| m.advance)
            .unwrap_or(t.len() as f32 * font.size * 0.6)
    };
    let find_w = measure(ctx, &find_text);
    let repl_w = replace_text
        .as_ref()
        .map(|t| measure(ctx, t))
        .unwrap_or(0.0);
    let hint_w = if s.replace.is_some() {
        measure(ctx, "Tab switch · Enter one · ^Enter all")
    } else {
        0.0
    };
    let content_w = find_w.max(repl_w).max(hint_w);

    let pad = 10.0;
    let rows = if s.replace.is_some() { 3.0 } else { 1.0 };
    let bw = (content_w + pad * 2.0 + 16.0).min(width - 24.0);
    let bh = line_h * rows + 6.0;
    let panel = top_right_rect(width, bw, bh, 11.0);
    let (bx, by) = (panel.x, panel.y);
    draw_panel(ctx, panel, 8.0)?;

    // Find line.
    let bl = baseline_in_rect(by + 3.0, line_h, ascent, descent);
    let prompt_w = measure(ctx, "find: ");
    let find_label_color = if s.focus_replace {
        palette().picker_detail
    } else {
        palette().picker_prompt
    };
    ctx.draw_text_with_font(
        "find: ",
        Point::new(bx + pad, bl),
        font,
        &solid(find_label_color),
    )?;
    let rest = format!("{}{}", s.query, count);
    ctx.draw_text_with_font(
        &rest,
        Point::new(bx + pad + prompt_w, bl),
        font,
        &solid(palette().picker_fg),
    )?;

    // Replace line + hint.
    if let Some(replace_text) = replace_text {
        let bl2 = baseline_in_rect(by + 3.0 + line_h, line_h, ascent, descent);
        let rprompt_w = measure(ctx, "replace: ");
        let repl_label_color = if s.focus_replace {
            palette().picker_prompt
        } else {
            palette().picker_detail
        };
        ctx.draw_text_with_font(
            "replace: ",
            Point::new(bx + pad, bl2),
            font,
            &solid(repl_label_color),
        )?;
        let rval = s.replace.clone().unwrap_or_default();
        let _ = replace_text;
        ctx.draw_text_with_font(
            &rval,
            Point::new(bx + pad + rprompt_w, bl2),
            font,
            &solid(palette().picker_fg),
        )?;

        let bl3 = baseline_in_rect(by + 3.0 + line_h * 2.0, line_h, ascent, descent);
        ctx.draw_text_with_font(
            "Tab switch · Enter one · ^Enter all",
            Point::new(bx + pad, bl3),
            font,
            &solid(palette().picker_detail),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozone_editor::Workspace;

    fn ws_with(text: &str) -> Workspace {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text(text);
        ws
    }

    fn search_state(ws: &mut Workspace) -> SearchState {
        let namespace = ws.decorations_mut().namespace();
        SearchState::new(false, namespace)
    }

    #[test]
    fn replace_all_replaces_every_match() {
        let mut ws = ws_with("foo bar foo baz foo");
        let mut s = search_state(&mut ws);
        s.query = "foo".into();
        s.replace = Some("X".into());
        search_recompute(&mut s, &mut ws);
        assert_eq!(s.matches.len(), 3);
        search_replace_all(&mut s, &mut ws);
        assert_eq!(ws.active_buffer().unwrap().text(), "X bar X baz X");
        assert!(s.matches.is_empty());
    }

    #[test]
    fn replace_current_replaces_one_then_recomputes() {
        let mut ws = ws_with("aa aa aa");
        let mut s = search_state(&mut ws);
        s.query = "aa".into();
        s.replace = Some("b".into());
        search_recompute(&mut s, &mut ws);
        s.current = 0;
        search_replace_current(&mut s, &mut ws);
        assert_eq!(ws.active_buffer().unwrap().text(), "b aa aa");
        assert_eq!(s.matches.len(), 2);
    }

    #[test]
    fn case_insensitive_replace_keeps_text_length_invariant() {
        let mut ws = ws_with("Foo FOO foo");
        let mut s = search_state(&mut ws); // case-insensitive
        s.query = "foo".into();
        s.replace = Some("ba".into());
        search_recompute(&mut s, &mut ws);
        assert_eq!(s.matches.len(), 3);
        search_replace_all(&mut s, &mut ws);
        assert_eq!(ws.active_buffer().unwrap().text(), "ba ba ba");
    }

    #[test]
    fn recompute_publishes_search_decorations() {
        let mut ws = ws_with("one two one");
        let mut s = search_state(&mut ws);
        s.query = "one".into();
        search_recompute(&mut s, &mut ws);

        let buffer = ws.active_buffer().unwrap().id;
        let decorations = ws.decorations().all(buffer);
        assert_eq!(decorations.len(), 2);
        assert!(matches!(
            decorations[0].kind,
            DecorationKind::Highlight(HlRole::SearchCurrent)
        ));
        assert!(matches!(
            decorations[1].kind,
            DecorationKind::Highlight(HlRole::Search)
        ));

        close_search(&s, &mut ws);
        assert!(ws.decorations().all(buffer).is_empty());
    }
}
