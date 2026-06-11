//! LSP completion popup.
//!
//! Opened by [`crate::lsp::Lsp::take_completion_result`] once a
//! `textDocument/completion` response arrives. Up/Down move the selection,
//! Enter/Tab insert the selected item (replacing the typed prefix), and
//! Escape — or any other key — closes the popup. Other keys are reported as
//! [`CompletionKeyResult::Closed`] so the caller still applies them normally
//! (typing continues; a fresh `ctrl+space` re-requests with the new prefix).

use aurea::AureaResult;
use aurea::render::{DrawingContext, Rect};
use ozone_buffer::{BufferId, Pos};
use ozone_config::Config;
use ozone_editor::{EditorEvent, Workspace};
use ozone_lsp::CompletionItem;

use crate::components::{ListRow, draw_list, draw_panel};
use crate::editor_font;
use crate::layout::STATUS_H;
use crate::lsp::CompletionResult;

/// Runtime state for the completion popup.
pub(crate) struct CompletionState {
    items: Vec<CompletionItem>,
    selected: usize,
    buffer_id: BufferId,
    /// Start of the identifier prefix being completed; `commit` replaces
    /// `[anchor, cursor)` with the selected item's `insert_text`.
    anchor: Pos,
}

/// Outcome of [`handle_completion_key`].
pub(crate) enum CompletionKeyResult {
    /// The key was consumed by the popup (selection moved, or an item was
    /// inserted and the popup closed).
    Handled,
    /// The popup closed (or was already closed) without consuming the key —
    /// the caller should still apply it normally.
    Closed,
}

impl CompletionState {
    pub(crate) fn new(result: CompletionResult) -> Self {
        Self {
            items: result.items,
            selected: 0,
            buffer_id: result.buffer_id,
            anchor: result.anchor,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
    }

    /// Replace `[anchor, cursor)` with the selected item's `insert_text` and
    /// move the cursor to the end of it. No-op if the cursor moved off the
    /// anchor line/before it (e.g. the buffer changed underneath the popup).
    fn commit(&self, ws: &mut Workspace) -> bool {
        let Some(item) = self.items.get(self.selected) else {
            return false;
        };
        let Some(view) = ws.active_view() else {
            return false;
        };
        if view.buffer_id != self.buffer_id {
            return false;
        }
        let cursor = view.cursor;
        if cursor.line != self.anchor.line || cursor.col < self.anchor.col {
            return false;
        }

        let delete_delta = if cursor.col > self.anchor.col {
            ws.buffers
                .get_mut(&self.buffer_id)
                .map(|buf| buf.delete_span(self.anchor, cursor))
        } else {
            None
        };
        let Some(insert_delta) = ws
            .buffers
            .get_mut(&self.buffer_id)
            .map(|buf| buf.insert(self.anchor, &item.insert_text))
        else {
            return false;
        };

        let new_col = self.anchor.col + item.insert_text.len();
        let view_id = ws.active_view_id;
        if let Some(view) = ws.active_view_mut() {
            view.cursor = Pos::new(self.anchor.line, new_col);
            view.col_memory = view.cursor.col;
            view.scroll_to_cursor(view.page_height.max(1));
        }

        if let Some(delta) = delete_delta {
            ws.emit(EditorEvent::BufferChanged {
                id: self.buffer_id,
                delta,
            });
        }
        ws.emit(EditorEvent::BufferChanged {
            id: self.buffer_id,
            delta: insert_delta,
        });
        if let Some(view_id) = view_id {
            ws.emit(EditorEvent::CursorMoved {
                view_id,
                pos: Pos::new(self.anchor.line, new_col),
            });
        }
        true
    }
}

/// Handle a key while the completion popup is open. Returns
/// [`CompletionKeyResult::Closed`] if `completion` is `None` on entry.
pub(crate) fn handle_completion_key(
    key: aurea::KeyCode,
    completion: &mut Option<CompletionState>,
    ws: &mut Workspace,
) -> CompletionKeyResult {
    use aurea::KeyCode::*;
    let Some(c) = completion.as_mut() else {
        return CompletionKeyResult::Closed;
    };
    match key {
        Escape => {
            *completion = None;
            CompletionKeyResult::Handled
        }
        Up => {
            c.move_sel(-1);
            CompletionKeyResult::Handled
        }
        Down => {
            c.move_sel(1);
            CompletionKeyResult::Handled
        }
        Enter | Tab => {
            let c = completion.take().expect("checked above");
            c.commit(ws);
            CompletionKeyResult::Handled
        }
        _ => {
            *completion = None;
            CompletionKeyResult::Closed
        }
    }
}

/// Render the completion popup: a small panel above the status bar listing
/// items, the selection highlighted (label + right-aligned detail).
pub(crate) fn draw_completion(
    ctx: &mut dyn DrawingContext,
    c: &CompletionState,
    config: &Config,
) -> AureaResult<()> {
    if c.items.is_empty() {
        return Ok(());
    }
    let w = ctx.width() as f32;
    let h = ctx.height() as f32;
    let font = editor_font(config);
    let line_h = (font.size * 1.6).max(16.0);
    let pad = 10.0;
    let radius = 8.0;

    let m = ctx.measure_text("M", &font).ok();
    let ascent = m.as_ref().map(|x| x.ascent).unwrap_or(font.size * 0.8);
    let descent = m.as_ref().map(|x| x.descent).unwrap_or(font.size * 0.2);
    let measure = |ctx: &mut dyn DrawingContext, t: &str| {
        ctx.measure_text(t, &font)
            .map(|x| x.advance)
            .unwrap_or(t.len() as f32 * font.size * 0.6)
    };

    let max_rows = 10usize;
    let start = if c.selected >= max_rows {
        c.selected + 1 - max_rows
    } else {
        0
    };
    let shown: Vec<usize> = (start..c.items.len()).take(max_rows).collect();

    let content_w = shown.iter().fold(0.0_f32, |acc, &i| {
        let item = &c.items[i];
        let mut row_w = measure(ctx, &item.label);
        if let Some(detail) = &item.detail {
            row_w += measure(ctx, detail) + 24.0;
        }
        acc.max(row_w)
    });

    let margin = 12.0;
    let pw = (content_w + pad * 2.0 + 16.0).clamp(160.0, w - margin * 2.0);
    let ph = shown.len() as f32 * line_h + pad;
    let panel = Rect::new(margin, (h - STATUS_H - ph - 6.0).max(6.0), pw, ph);

    draw_panel(ctx, panel, radius)?;

    let rows: Vec<ListRow> = shown
        .iter()
        .map(|&idx| ListRow {
            primary: &c.items[idx].label,
            detail: c.items[idx].detail.as_deref().unwrap_or(""),
        })
        .collect();
    let sel = c.selected.checked_sub(start);
    draw_list(
        ctx,
        panel.x,
        panel.y + pad / 2.0,
        pw,
        line_h,
        pad,
        &rows,
        sel,
        &font,
        ascent,
        descent,
    )?;
    Ok(())
}
