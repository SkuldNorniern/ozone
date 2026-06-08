use ozone_buffer::Pos;

use crate::events::EditorEvent;

use super::{CommandRegistry, emit_cursor_moved, leading_whitespace, trailing_whitespace_ranges};

pub(super) fn register_edit_commands(reg: &mut CommandRegistry) {
    // --- editing ---

    reg.register(
        "edit.delete-char-backward",
        "Delete character before cursor",
        |ctx| {
            let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
            let cursor = view.cursor;
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();

            if cursor.col > 0 {
                let offset = buf.pos_to_offset(cursor) - 1;
                let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
                let delta = buf.delete_at(offset, 1);
                let cursor = {
                    let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
                    view.cursor.col -= 1;
                    view.col_memory = view.cursor.col;
                    view.cursor
                };
                if let Some(delta) = delta {
                    ctx.workspace.emit(EditorEvent::BufferChanged {
                        id: ctx.buffer_id,
                        delta,
                    });
                }
                ctx.workspace.emit(EditorEvent::CursorMoved {
                    view_id: ctx.view_id,
                    pos: cursor,
                });
            } else if cursor.line > 0 {
                let prev_line_len = buf.line_len(cursor.line - 1);
                let offset = buf.pos_to_offset(Pos::new(cursor.line - 1, prev_line_len));
                let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
                let delta = buf.delete_at(offset, 1);
                let cursor = {
                    let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
                    view.cursor = Pos::new(cursor.line - 1, prev_line_len);
                    view.col_memory = view.cursor.col;
                    view.cursor
                };
                if let Some(delta) = delta {
                    ctx.workspace.emit(EditorEvent::BufferChanged {
                        id: ctx.buffer_id,
                        delta,
                    });
                }
                ctx.workspace.emit(EditorEvent::CursorMoved {
                    view_id: ctx.view_id,
                    pos: cursor,
                });
            }
        },
    );

    reg.register(
        "edit.delete-char-forward",
        "Delete character after cursor",
        |ctx| {
            let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
            let cursor = view.cursor;
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let offset = buf.pos_to_offset(cursor);
            let total = buf.text().len();
            if offset < total {
                let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
                if let Some(delta) = buf.delete_at(offset, 1) {
                    ctx.workspace.emit(EditorEvent::BufferChanged {
                        id: ctx.buffer_id,
                        delta,
                    });
                }
            }
        },
    );

    reg.register(
        "edit.insert-newline",
        "Insert a newline, preserving indentation",
        |ctx| {
            let cursor = ctx.workspace.views.get(&ctx.view_id).unwrap().cursor;
            let indent_unit = ctx.workspace.indent_for(ctx.buffer_id).unit();

            let indent = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let line_text = buf.line(cursor.line).unwrap_or_default();
                let lead = leading_whitespace(&line_text);
                let before = &line_text[..cursor.col.min(line_text.len())];
                let opens_block = before.trim_end().ends_with(['{', '(', '[']);
                if opens_block {
                    format!("{lead}{indent_unit}")
                } else {
                    lead.to_string()
                }
            };

            let insert = format!("\n{indent}");
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            let delta = buf.insert(cursor, &insert);
            let cursor = {
                let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
                view.cursor = Pos::new(cursor.line + 1, indent.len());
                view.col_memory = view.cursor.col;
                view.cursor
            };
            ctx.workspace.emit(EditorEvent::BufferChanged {
                id: ctx.buffer_id,
                delta,
            });
            ctx.workspace.emit(EditorEvent::CursorMoved {
                view_id: ctx.view_id,
                pos: cursor,
            });
        },
    );

    reg.register("edit.undo", "Undo last edit", |ctx| {
        let result = ctx
            .workspace
            .buffers
            .get_mut(&ctx.buffer_id)
            .and_then(|buf| buf.undo_with_delta());
        if let Some((pos, delta)) = result {
            ctx.workspace.emit(EditorEvent::BufferChanged {
                id: ctx.buffer_id,
                delta,
            });
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.cursor = pos;
            view.col_memory = pos.col;
            emit_cursor_moved(ctx, old);
        }
    });

    reg.register("edit.redo", "Redo last undone edit", |ctx| {
        let result = ctx
            .workspace
            .buffers
            .get_mut(&ctx.buffer_id)
            .and_then(|buf| buf.redo_with_delta());
        if let Some((pos, delta)) = result {
            ctx.workspace.emit(EditorEvent::BufferChanged {
                id: ctx.buffer_id,
                delta,
            });
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.cursor = pos;
            view.col_memory = pos.col;
            emit_cursor_moved(ctx, old);
        }
    });

    reg.register(
        "edit.trim-trailing-whitespace",
        "Trim trailing spaces and tabs",
        |ctx| {
            let ranges = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                trailing_whitespace_ranges(&buf.text())
            };
            if ranges.is_empty() {
                return;
            }

            let mut deltas = Vec::new();
            let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
            for (offset, len) in ranges.into_iter().rev() {
                if let Some(delta) = buf.delete_at(offset, len) {
                    deltas.push(delta);
                }
            }
            for delta in deltas {
                ctx.workspace.emit(EditorEvent::BufferChanged {
                    id: ctx.buffer_id,
                    delta,
                });
            }
        },
    );
}
