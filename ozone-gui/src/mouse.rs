//! Mouse / pointer state + hit-testing for the run loop.
//!
//! `MouseState` retains the last cursor position (Aurea button events carry
//! their own coordinates, but the wheel handler needs the last move position to
//! pick which pane to scroll/focus). `handle_editor_click` maps a click in
//! window coordinates to a buffer position (placing the cursor / extending a
//! selection), using the same geometry the renderer draws with.
//!
//! This module is the deliberate home for the forthcoming unified pointer model
//! in `docs/aurea-pointer-roadmap.md`: when Aurea ships `PointerEvent` + element
//! capture, the pressed-button set, the drag-selection anchor, the capture
//! target, and the active cursor shape all belong here — so press-drag-release
//! selection and double/triple-click land without re-plumbing the event loop.

use aurea::render::Rect;
use ozone_buffer::{BufferKind, Pos, Span};
use ozone_config::{Config, LineNumbers};
use ozone_editor::{EditorEvent, ViewId, Workspace, buffer_language};

use crate::layout::{
    EDITOR_TOP_PAD, PAD, STATUS_H, gutter_width, max_scroll_line, pane_at, pane_rect,
};
use ozone_editor::fold;
use ozone_syntax::fold_line_ranges;

/// Run-loop pointer state. See the module docs for the planned growth.
#[derive(Default)]
pub(crate) struct MouseState {
    /// Last cursor position in window coordinates, or `None` before the first
    /// move event. Currently consumed only by wheel pane targeting.
    pos: Option<(f32, f32)>,
    selection_drag: Option<(ViewId, Pos)>,
    scrollbar_drag: Option<(ViewId, f32)>,
}

impl MouseState {
    /// Record a move. Window coordinates.
    pub(crate) fn moved(&mut self, x: f32, y: f32) {
        self.pos = Some((x, y));
    }

    /// The last known cursor position, if any.
    pub(crate) fn pos(&self) -> Option<(f32, f32)> {
        self.pos
    }

    pub(crate) fn begin_selection_drag(&mut self, view_id: ViewId, anchor: Pos) {
        self.scrollbar_drag = None;
        self.selection_drag = Some((view_id, anchor));
    }

    pub(crate) fn end_selection_drag(&mut self) {
        self.selection_drag = None;
    }

    pub(crate) fn selection_drag(&self) -> Option<(ViewId, Pos)> {
        self.selection_drag
    }

    pub(crate) fn begin_scrollbar_drag(&mut self, view_id: ViewId, grab_y: f32) {
        self.selection_drag = None;
        self.scrollbar_drag = Some((view_id, grab_y));
    }

    pub(crate) fn end_scrollbar_drag(&mut self) {
        self.scrollbar_drag = None;
    }

    pub(crate) fn scrollbar_drag(&self) -> Option<(ViewId, f32)> {
        self.scrollbar_drag
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollbarPress {
    pub view_id: ViewId,
    pub grab_y: f32,
    pub changed: bool,
}

/// Map a left-click at window `(x, y)` to a cursor position: focus the pane,
/// place the cursor, and (when `extend_selection`) grow an ordered selection
/// from the old cursor. Returns whether anything changed. Mirrors the renderer's
/// pane/gutter/line geometry so a click lands on the glyph under the pointer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_editor_click(
    ws: &mut Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    char_w: f32,
    extend_selection: bool,
    click_count: u8,
) -> bool {
    if let Some((view_id, new_pos)) =
        editor_text_position_at(ws, config, x, y, width, height, char_w, None)
    {
        let old_pos = ws.views.get(&view_id).map(|view| view.cursor);
        let selection = match (click_count, old_pos) {
            (3.., _) => line_span_at(ws, view_id, new_pos),
            (2, _) => word_span_at(ws, view_id, new_pos),
            (_, Some(old_pos)) if extend_selection && old_pos != new_pos => {
                Some(ordered_span(old_pos, new_pos))
            }
            _ => None,
        };
        let final_cursor = {
            let Some(view) = ws.views.get_mut(&view_id) else {
                return false;
            };
            view.cursor = new_pos;
            view.col_memory = new_pos.col;
            view.selection = selection;
            if let Some(selection) = view.selection {
                view.cursor = selection.end;
                view.col_memory = selection.end.col;
            }
            view.cursor
        };
        ws.active_view_id = Some(view_id);
        ws.emit(EditorEvent::CursorMoved {
            view_id,
            pos: final_cursor,
        });
        return true;
    }

    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let target = ws
        .panes
        .as_ref()
        .and_then(|tree| pane_at(tree, editor_rect, x, y))
        .or_else(|| ws.active_view_id.map(|id| (id, editor_rect)));
    let Some((view_id, _rect)) = target else {
        return false;
    };
    let Some(buffer_id) = ws.views.get(&view_id).map(|view| view.buffer_id) else {
        return false;
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return false;
    };
    if matches!(buf.kind, BufferKind::Image(_) | BufferKind::Terminal) {
        ws.active_view_id = Some(view_id);
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_editor_drag(
    ws: &mut Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    char_w: f32,
    anchor_view: ViewId,
    anchor: Pos,
) -> bool {
    let Some((view_id, new_pos)) =
        editor_text_position_at(ws, config, x, y, width, height, char_w, Some(anchor_view))
    else {
        return false;
    };
    if view_id != anchor_view {
        return false;
    }

    let Some(view) = ws.views.get_mut(&view_id) else {
        return false;
    };
    let desired_selection = if anchor == new_pos {
        None
    } else {
        Some(ordered_span(anchor, new_pos))
    };
    let changed = view.cursor != new_pos || view.selection != desired_selection;
    if !changed {
        return false;
    }
    view.cursor = new_pos;
    view.col_memory = new_pos.col;
    view.selection = desired_selection;
    ws.active_view_id = Some(view_id);
    ws.emit(EditorEvent::CursorMoved {
        view_id,
        pos: new_pos,
    });
    true
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_scrollbar_press(
    ws: &mut Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Option<ScrollbarPress> {
    let (view_id, thumb_y, thumb_h) = scrollbar_at(ws, config, x, y, width, height)?;
    let grab_y = if y >= thumb_y && y <= thumb_y + thumb_h {
        y - thumb_y
    } else {
        thumb_h / 2.0
    };
    let changed = handle_scrollbar_drag(ws, config, y, width, height, view_id, grab_y);
    Some(ScrollbarPress {
        view_id,
        grab_y,
        changed,
    })
}

pub(crate) fn handle_scrollbar_drag(
    ws: &mut Workspace,
    config: &Config,
    y: f32,
    width: f32,
    height: f32,
    view_id: ViewId,
    grab_y: f32,
) -> bool {
    let Some((rect, line_count, viewport_lines, line_h)) =
        scrollbar_view_metrics(ws, config, width, height, view_id)
    else {
        return false;
    };
    let max_scroll = max_scroll_line(line_count, viewport_lines as usize);
    if max_scroll == 0 {
        return false;
    }
    let track_h = rect.height;
    let thumb_h = (track_h * viewport_lines / line_count as f32).clamp(24.0, track_h);
    let travel = (track_h - thumb_h).max(1.0);
    let thumb_y = (y - grab_y).clamp(rect.y, rect.y + travel);
    let total = ((thumb_y - rect.y) / travel) * max_scroll as f32 * line_h;
    let scroll_line = (total / line_h).floor() as usize;
    let scroll_y = total - scroll_line as f32 * line_h;

    let Some(view) = ws.views.get_mut(&view_id) else {
        return false;
    };
    let scroll_line = scroll_line.min(max_scroll);
    let scroll_y = if scroll_line >= max_scroll {
        0.0
    } else {
        scroll_y
    };
    let changed = view.scroll_line != scroll_line || (view.scroll_y - scroll_y).abs() > 0.01;
    if changed {
        view.scroll_line = scroll_line;
        view.scroll_y = scroll_y;
        ws.active_view_id = Some(view_id);
    }
    changed
}

/// Click on the gutter fold indicator or the inline fold badge → toggle fold.
/// Returns `true` if a fold was toggled (caller should redraw).
pub(crate) fn handle_fold_click(
    ws: &mut Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    char_w: f32,
) -> bool {
    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let Some((view_id, rect)) = ws
        .panes
        .as_ref()
        .and_then(|tree| pane_at(tree, editor_rect, x, y))
        .or_else(|| ws.active_view_id.map(|id| (id, editor_rect)))
    else {
        return false;
    };
    let Some(buffer_id) = ws.views.get(&view_id).map(|v| v.buffer_id) else {
        return false;
    };
    let Some(buf) = ws.buffers.get(&buffer_id) else {
        return false;
    };
    if matches!(buf.kind, BufferKind::Image(_) | BufferKind::Terminal) {
        return false;
    }
    let line_count = buf.line_count();
    if line_count == 0 {
        return false;
    }
    let line_numbers = match buf.kind {
        BufferKind::Search | BufferKind::References | BufferKind::FileTree => LineNumbers::Off,
        _ => ws
            .buffer_local(buffer_id)
            .and_then(|local| local.line_numbers)
            .unwrap_or(config.editor.line_numbers),
    };
    let gutter_w = gutter_width(line_count, char_w.max(1.0), line_numbers);
    if gutter_w == 0.0 || x >= rect.x + gutter_w {
        return false;
    }
    let line_h = (config.editor.font_size * config.editor.line_height).max(1.0);
    let (scroll, scroll_y) = ws
        .views
        .get(&view_id)
        .map(|v| (v.scroll_line, v.scroll_y))
        .unwrap_or((0, 0.0));
    let relative_y = (y - rect.y - EDITOR_TOP_PAD).max(0.0);
    let line_idx =
        (scroll + ((relative_y + scroll_y) / line_h).floor() as usize).min(line_count - 1);

    let lang = buffer_language(buf);
    let struct_ranges = fold_line_ranges(lang, &buf.text());
    let header = if struct_ranges.is_empty() {
        if fold::is_visual_fold_header(buf, line_idx) {
            Some(line_idx)
        } else {
            fold::header_for(buf, line_idx)
        }
    } else if fold::structural_is_foldable_at(&struct_ranges, line_idx) {
        Some(line_idx)
    } else {
        fold::structural_header_for(&struct_ranges, line_idx)
    };
    let Some(header) = header else {
        return false;
    };
    let Some(view) = ws.views.get_mut(&view_id) else {
        return false;
    };
    if !view.folds.remove(&header) {
        view.folds.insert(header);
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn editor_text_position_at(
    ws: &Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    char_w: f32,
    capture_view: Option<ViewId>,
) -> Option<(ViewId, Pos)> {
    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let target = if let Some(view_id) = capture_view {
        ws.panes
            .as_ref()
            .and_then(|tree| pane_rect(tree, editor_rect, view_id))
            .or((ws.active_view_id == Some(view_id)).then_some(editor_rect))
            .map(|rect| (view_id, rect))
    } else {
        ws.panes
            .as_ref()
            .and_then(|tree| pane_at(tree, editor_rect, x, y))
            .or_else(|| ws.active_view_id.map(|id| (id, editor_rect)))
    };
    let (view_id, rect) = target?;
    let buffer_id = ws.views.get(&view_id).map(|view| view.buffer_id)?;
    let buf = ws.buffers.get(&buffer_id)?;
    if matches!(buf.kind, BufferKind::Image(_) | BufferKind::Terminal) {
        return None;
    }
    let line_count = buf.line_count();
    if line_count == 0 {
        return None;
    }
    let line_numbers = match buf.kind {
        BufferKind::Search | BufferKind::References | BufferKind::FileTree => LineNumbers::Off,
        _ => ws
            .buffer_local(buffer_id)
            .and_then(|local| local.line_numbers)
            .unwrap_or(config.editor.line_numbers),
    };
    let line_h = (config.editor.font_size * config.editor.line_height).max(1.0);
    let (scroll, scroll_y) = ws
        .views
        .get(&view_id)
        .map(|v| (v.scroll_line, v.scroll_y))
        .unwrap_or((0, 0.0));
    let relative_y = (y - rect.y - EDITOR_TOP_PAD).max(0.0);
    let line = (scroll + ((relative_y + scroll_y) / line_h).floor() as usize).min(line_count - 1);
    let gutter_w = gutter_width(line_count, char_w.max(1.0), line_numbers);
    let text_x = rect.x + gutter_w + PAD;
    let raw_col = if x <= text_x {
        0
    } else {
        ((x - text_x) / char_w.max(1.0)).round() as usize
    };
    let line_text = buf.line(line).unwrap_or_default();
    let mut col = raw_col.min(line_text.len());
    while col > 0 && !line_text.is_char_boundary(col) {
        col -= 1;
    }
    Some((view_id, Pos::new(line, col)))
}

fn scrollbar_at(
    ws: &Workspace,
    config: &Config,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Option<(ViewId, f32, f32)> {
    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let (view_id, _rect) = ws
        .panes
        .as_ref()
        .and_then(|tree| pane_at(tree, editor_rect, x, y))
        .or_else(|| ws.active_view_id.map(|id| (id, editor_rect)))?;
    let (rect, line_count, viewport_lines, line_h) =
        scrollbar_view_metrics(ws, config, width, height, view_id)?;
    if y < rect.y || y > rect.y + rect.height {
        return None;
    }
    let track_h = rect.height;
    if line_count as f32 <= viewport_lines {
        return None;
    }
    let bar_x = rect.x + rect.width - 4.0;
    if x < bar_x - 5.0 || x > rect.x + rect.width {
        return None;
    }
    let thumb_h = (track_h * viewport_lines / line_count as f32).clamp(24.0, track_h);
    let max_scroll = max_scroll_line(line_count, viewport_lines as usize);
    let (scroll, scroll_y) = ws
        .views
        .get(&view_id)
        .map(|v| (v.scroll_line, v.scroll_y))?;
    let t = if max_scroll > 0 {
        ((scroll as f32 + scroll_y / line_h) / max_scroll as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let thumb_y = rect.y + t * (track_h - thumb_h);
    Some((view_id, thumb_y, thumb_h))
}

fn scrollbar_view_metrics(
    ws: &Workspace,
    config: &Config,
    width: f32,
    height: f32,
    view_id: ViewId,
) -> Option<(Rect, usize, f32, f32)> {
    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let rect = ws
        .panes
        .as_ref()
        .and_then(|tree| pane_rect(tree, editor_rect, view_id))
        .or((ws.active_view_id == Some(view_id)).then_some(editor_rect))?;
    let buffer_id = ws.views.get(&view_id)?.buffer_id;
    let line_count = ws.buffers.get(&buffer_id)?.line_count();
    let line_h = (config.editor.font_size * config.editor.line_height).max(1.0);
    let content_h = (rect.height - EDITOR_TOP_PAD).max(0.0);
    let viewport_lines = (content_h / line_h).max(1.0);
    Some((rect, line_count, viewport_lines, line_h))
}

fn ordered_span(a: Pos, b: Pos) -> Span {
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    Span { start, end }
}

fn word_span_at(ws: &Workspace, view_id: ViewId, pos: Pos) -> Option<Span> {
    let buffer_id = ws.views.get(&view_id)?.buffer_id;
    let line = ws.buffers.get(&buffer_id)?.line(pos.line)?;
    if line.is_empty() {
        return None;
    }
    let bytes = line.as_bytes();
    let mut col = pos.col.min(bytes.len());
    while col > 0 && !line.is_char_boundary(col) {
        col -= 1;
    }
    let mut anchor = col;
    if anchor == bytes.len() || !is_word_byte(bytes[anchor]) {
        if anchor == 0 || !is_word_byte(bytes[anchor - 1]) {
            return None;
        }
        anchor -= 1;
    }
    let mut start = anchor;
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < bytes.len() && is_word_byte(bytes[end]) {
        end += 1;
    }
    Some(Span::new(
        Pos::new(pos.line, start),
        Pos::new(pos.line, end),
    ))
}

fn line_span_at(ws: &Workspace, view_id: ViewId, pos: Pos) -> Option<Span> {
    let buffer_id = ws.views.get(&view_id)?.buffer_id;
    let len = ws.buffers.get(&buffer_id)?.line_len(pos.line);
    Some(Span::new(Pos::new(pos.line, 0), Pos::new(pos.line, len)))
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_workspace(text: &str) -> Workspace {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text(text);
        ws
    }

    #[test]
    fn click_places_cursor_using_view_coordinates() {
        let mut ws = text_workspace("alpha\nbravo\ncharlie");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        config.editor.font_size = 10.0;
        config.editor.line_height = 2.0;

        assert!(handle_editor_click(
            &mut ws,
            &config,
            PAD + 3.0 * 8.0,
            EDITOR_TOP_PAD + 1.2 * 20.0,
            800.0,
            600.0,
            8.0,
            false,
            1,
        ));
        assert_eq!(
            ws.active_view().unwrap().cursor,
            ozone_buffer::Pos::new(1, 3)
        );
    }

    #[test]
    fn shift_click_creates_ordered_selection() {
        let mut ws = text_workspace("abcdef");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        ws.active_view_mut().unwrap().cursor = ozone_buffer::Pos::new(0, 5);

        assert!(handle_editor_click(
            &mut ws,
            &config,
            PAD + 2.0 * 8.0,
            EDITOR_TOP_PAD,
            800.0,
            600.0,
            8.0,
            true,
            1,
        ));
        let selection = ws.active_view().unwrap().selection.unwrap();
        assert_eq!(selection.start, ozone_buffer::Pos::new(0, 2));
        assert_eq!(selection.end, ozone_buffer::Pos::new(0, 5));
    }

    #[test]
    fn double_click_selects_word() {
        let mut ws = text_workspace("alpha beta_2!");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;

        assert!(handle_editor_click(
            &mut ws,
            &config,
            PAD + 8.0 * 8.0,
            EDITOR_TOP_PAD,
            800.0,
            600.0,
            8.0,
            false,
            2,
        ));
        let view = ws.active_view().unwrap();
        assert_eq!(view.cursor, ozone_buffer::Pos::new(0, 12));
        assert_eq!(
            view.selection.unwrap(),
            ozone_buffer::Span::new(ozone_buffer::Pos::new(0, 6), ozone_buffer::Pos::new(0, 12))
        );
    }

    #[test]
    fn triple_click_selects_line() {
        let mut ws = text_workspace("alpha\nbravo");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        config.editor.font_size = 10.0;
        config.editor.line_height = 2.0;

        assert!(handle_editor_click(
            &mut ws,
            &config,
            PAD + 2.0 * 8.0,
            EDITOR_TOP_PAD + 1.2 * 20.0,
            800.0,
            600.0,
            8.0,
            false,
            3,
        ));
        let view = ws.active_view().unwrap();
        assert_eq!(view.cursor, ozone_buffer::Pos::new(1, 5));
        assert_eq!(
            view.selection.unwrap(),
            ozone_buffer::Span::new(ozone_buffer::Pos::new(1, 0), ozone_buffer::Pos::new(1, 5))
        );
    }

    #[test]
    fn drag_extends_selection_from_anchor() {
        let mut ws = text_workspace("abcdef");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        let view_id = ws.active_view_id.unwrap();

        assert!(handle_editor_drag(
            &mut ws,
            &config,
            PAD + 5.0 * 8.0,
            EDITOR_TOP_PAD,
            800.0,
            600.0,
            8.0,
            view_id,
            ozone_buffer::Pos::new(0, 1),
        ));
        let view = ws.active_view().unwrap();
        assert_eq!(view.cursor, ozone_buffer::Pos::new(0, 5));
        assert_eq!(
            view.selection.unwrap(),
            ozone_buffer::Span::new(ozone_buffer::Pos::new(0, 1), ozone_buffer::Pos::new(0, 5))
        );
    }

    #[test]
    fn drag_collapsed_to_anchor_clears_selection() {
        let mut ws = text_workspace("abcdef");
        let mut config = Config::default_config();
        config.editor.line_numbers = LineNumbers::Off;
        let view_id = ws.active_view_id.unwrap();
        ws.active_view_mut().unwrap().selection = Some(ozone_buffer::Span::new(
            ozone_buffer::Pos::new(0, 1),
            ozone_buffer::Pos::new(0, 5),
        ));

        assert!(handle_editor_drag(
            &mut ws,
            &config,
            PAD + 1.0 * 8.0,
            EDITOR_TOP_PAD,
            800.0,
            600.0,
            8.0,
            view_id,
            ozone_buffer::Pos::new(0, 1),
        ));
        assert_eq!(
            ws.active_view().unwrap().cursor,
            ozone_buffer::Pos::new(0, 1)
        );
        assert!(ws.active_view().unwrap().selection.is_none());
    }

    #[test]
    fn scrollbar_press_scrolls_view() {
        let text = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ws = text_workspace(&text);
        let mut config = Config::default_config();
        config.editor.font_size = 10.0;
        config.editor.line_height = 2.0;

        let press = handle_scrollbar_press(&mut ws, &config, 798.0, 500.0, 800.0, 600.0)
            .expect("scrollbar hit");
        assert_eq!(press.view_id, ws.active_view_id.unwrap());
        assert!(press.changed);
        assert!(ws.active_view().unwrap().scroll_line > 0);
    }

    #[test]
    fn scrollbar_drag_clamps_to_bottom() {
        let text = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ws = text_workspace(&text);
        let mut config = Config::default_config();
        config.editor.font_size = 10.0;
        config.editor.line_height = 2.0;
        let view_id = ws.active_view_id.unwrap();

        assert!(handle_scrollbar_drag(
            &mut ws, &config, 10_000.0, 800.0, 600.0, view_id, 0.0,
        ));
        let view = ws.active_view().unwrap();
        assert_eq!(view.scroll_line, max_scroll_line(100, 28));
        assert_eq!(view.scroll_y, 0.0);
    }
}
