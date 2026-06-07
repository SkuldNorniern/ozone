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
use ozone_buffer::BufferKind;
use ozone_config::{Config, LineNumbers};
use ozone_editor::{EditorEvent, Workspace};

use crate::layout::{EDITOR_TOP_PAD, PAD, STATUS_H, gutter_width, pane_at};

/// Run-loop pointer state. See the module docs for the planned growth.
#[derive(Default)]
pub(crate) struct MouseState {
    /// Last cursor position in window coordinates, or `None` before the first
    /// move event. Currently consumed only by wheel pane targeting.
    pos: Option<(f32, f32)>,
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
) -> bool {
    let editor_rect = Rect::new(0.0, 0.0, width, (height - STATUS_H).max(0.0));
    let target = ws
        .panes
        .as_ref()
        .and_then(|tree| pane_at(tree, editor_rect, x, y))
        .or_else(|| ws.active_view_id.map(|id| (id, editor_rect)));
    let Some((view_id, rect)) = target else {
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

    let line_count = buf.line_count();
    if line_count == 0 {
        return false;
    }
    let line_numbers = match buf.kind {
        BufferKind::Search | BufferKind::References => LineNumbers::Off,
        _ => ws
            .buffer_local(buffer_id)
            .and_then(|local| local.line_numbers)
            .unwrap_or(config.editor.line_numbers),
    };
    let line_h = (config.editor.font_size * config.editor.line_height).max(1.0);
    let scroll = ws
        .views
        .get(&view_id)
        .map(|view| view.scroll_line)
        .unwrap_or(0);
    let relative_y = (y - rect.y - EDITOR_TOP_PAD).max(0.0);
    let line = (scroll + (relative_y / line_h).floor() as usize).min(line_count - 1);
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
    let new_pos = ozone_buffer::Pos::new(line, col);

    let Some(view) = ws.views.get_mut(&view_id) else {
        return false;
    };
    let old_pos = view.cursor;
    view.cursor = new_pos;
    view.col_memory = col;
    view.selection = if extend_selection && old_pos != new_pos {
        let (start, end) = if old_pos <= new_pos {
            (old_pos, new_pos)
        } else {
            (new_pos, old_pos)
        };
        Some(ozone_buffer::Span { start, end })
    } else {
        None
    };
    ws.active_view_id = Some(view_id);
    ws.emit(EditorEvent::CursorMoved {
        view_id,
        pos: new_pos,
    });
    true
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
        ));
        let selection = ws.active_view().unwrap().selection.unwrap();
        assert_eq!(selection.start, ozone_buffer::Pos::new(0, 2));
        assert_eq!(selection.end, ozone_buffer::Pos::new(0, 5));
    }
}
