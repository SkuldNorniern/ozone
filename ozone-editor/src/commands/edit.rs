use std::collections::HashMap;

use ozone_buffer::{BufferKind, Pos};
use taste::detect_language;

use crate::events::EditorEvent;

use super::{
    CommandContext, CommandRegistry, emit_cursor_moved, leading_whitespace, selection_line_range,
    trailing_whitespace_ranges,
};

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
        "edit.toggle-comment",
        "Toggle the line comment for the current line or selection",
        toggle_comment,
    );

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

fn toggle_comment(ctx: &mut CommandContext) {
    let Some(buf) = ctx.workspace.buffers.get(&ctx.buffer_id) else {
        return;
    };
    let lang = match &buf.kind {
        BufferKind::File(p) => detect_language(p),
        _ => None,
    };
    let Some(prefix) = lang.and_then(|l| l.comments().primary_line()) else {
        return;
    };

    let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
    let (start_line, end_line) = selection_line_range(view);

    let all_commented = (start_line..=end_line)
        .filter_map(|l| buf.line(l))
        .filter(|l| !l.trim().is_empty())
        .all(|l| l[leading_whitespace(&l).len()..].starts_with(prefix));

    let buf = ctx.workspace.buffers.get_mut(&ctx.buffer_id).unwrap();
    let mut deltas = Vec::new();
    let mut col_shift: HashMap<usize, isize> = HashMap::new();
    for line in start_line..=end_line {
        let Some(text) = buf.line(line) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let indent = leading_whitespace(&text).len();
        if all_commented {
            let after = &text[indent..];
            let mut remove_len = prefix.len();
            if after[prefix.len()..].starts_with(' ') {
                remove_len += 1;
            }
            let offset = buf.pos_to_offset(Pos::new(line, indent));
            if let Some(delta) = buf.delete_at(offset, remove_len) {
                deltas.push(delta);
                col_shift.insert(line, -(remove_len as isize));
            }
        } else {
            let insert = format!("{prefix} ");
            let delta = buf.insert(Pos::new(line, indent), &insert);
            col_shift.insert(line, insert.len() as isize);
            deltas.push(delta);
        }
    }

    for delta in deltas {
        ctx.workspace.emit(EditorEvent::BufferChanged {
            id: ctx.buffer_id,
            delta,
        });
    }

    let mut cursor_moved_from = None;
    if let Some(view) = ctx.workspace.views.get_mut(&ctx.view_id) {
        let shift = |pos: &mut Pos, shifts: &HashMap<usize, isize>| {
            if let Some(&d) = shifts.get(&pos.line) {
                pos.col = (pos.col as isize + d).max(0) as usize;
            }
        };
        let old_cursor = view.cursor;
        shift(&mut view.cursor, &col_shift);
        view.col_memory = view.cursor.col;
        if let Some(span) = view.selection.as_mut() {
            shift(&mut span.start, &col_shift);
            shift(&mut span.end, &col_shift);
        }
        if view.cursor != old_cursor {
            cursor_moved_from = Some(old_cursor);
        }
    }
    if let Some(old) = cursor_moved_from {
        emit_cursor_moved(ctx, old);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ozone_buffer::{BufferKind, Pos, Span};

    use crate::commands::register_defaults;
    use crate::workspace::Workspace;
    use crate::{CommandContext, CommandRegistry};

    fn rust_ws(text: &str) -> Workspace {
        let mut ws = Workspace::new();
        let buf = ws.active_buffer_mut().unwrap();
        buf.set_text(text);
        buf.kind = BufferKind::File(PathBuf::from("toggle.rs"));
        ws
    }

    fn run(ws: &mut Workspace) {
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let mut ctx = CommandContext::new(ws).unwrap();
        assert!(reg.execute("edit.toggle-comment", &mut ctx));
    }

    #[test]
    fn toggle_comment_adds_then_removes_line_prefix() {
        let mut ws = rust_ws("fn main() {}");
        ws.active_view_mut().unwrap().cursor = Pos::new(0, 3);

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("// fn main() {}"));
        assert_eq!(ws.active_view().unwrap().cursor, Pos::new(0, 6));

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("fn main() {}"));
        assert_eq!(ws.active_view().unwrap().cursor, Pos::new(0, 3));
    }

    #[test]
    fn toggle_comment_preserves_indentation() {
        let mut ws = rust_ws("    let x = 1;");
        ws.active_view_mut().unwrap().cursor = Pos::new(0, 0);

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("    // let x = 1;"));

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("    let x = 1;"));
    }

    #[test]
    fn toggle_comment_over_selection_comments_all_non_blank_lines() {
        let mut ws = rust_ws("a\n\nb\nc");
        ws.active_view_mut().unwrap().selection = Some(Span {
            start: Pos::new(0, 0),
            end: Pos::new(3, 1),
        });

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("// a"));
        assert_eq!(buf.line(1).as_deref(), Some(""));
        assert_eq!(buf.line(2).as_deref(), Some("// b"));
        assert_eq!(buf.line(3).as_deref(), Some("// c"));

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("a"));
        assert_eq!(buf.line(2).as_deref(), Some("b"));
        assert_eq!(buf.line(3).as_deref(), Some("c"));
    }

    #[test]
    fn toggle_comment_uncomments_without_trailing_space() {
        let mut ws = rust_ws("//no space");
        ws.active_view_mut().unwrap().cursor = Pos::new(0, 0);

        run(&mut ws);
        let buf = ws.active_buffer().unwrap();
        assert_eq!(buf.line(0).as_deref(), Some("no space"));
    }

    #[test]
    fn toggle_comment_no_op_for_scratch_buffer() {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text("plain text");
        ws.active_view_mut().unwrap().cursor = Pos::new(0, 0);

        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let mut ctx = CommandContext::new(&mut ws).unwrap();
        assert!(reg.execute("edit.toggle-comment", &mut ctx));

        assert_eq!(
            ws.active_buffer().unwrap().line(0).as_deref(),
            Some("plain text")
        );
    }
}
