use ozone_buffer::Pos;

use crate::ui::UiIntent;

use super::{CommandRegistry, emit_cursor_moved, max_scroll_line, word_backward, word_forward};

pub(super) fn register_cursor_commands(reg: &mut CommandRegistry) {
    // --- cursor movement ---

    reg.register(
        "cursor.move-left",
        "Move cursor one character left",
        |ctx| {
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.clear_selection();
            if view.cursor.col > 0 {
                view.cursor.col -= 1;
            } else if view.cursor.line > 0 {
                view.cursor.line -= 1;
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                view.cursor.col = buf.line_len(view.cursor.line);
            }
            view.col_memory = view.cursor.col;
            emit_cursor_moved(ctx, old);
        },
    );

    reg.register(
        "cursor.move-right",
        "Move cursor one character right",
        |ctx| {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.clear_selection();
            let line_len = buf.line_len(view.cursor.line);
            if view.cursor.col < line_len {
                view.cursor.col += 1;
            } else if view.cursor.line + 1 < buf.line_count() {
                view.cursor.line += 1;
                view.cursor.col = 0;
            }
            view.col_memory = view.cursor.col;
            emit_cursor_moved(ctx, old);
        },
    );

    reg.register("cursor.move-up", "Move cursor one line up", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        if view.cursor.line > 0 {
            view.cursor.line -= 1;
            let target_col = view.col_memory;
            view.cursor.col = target_col.min(buf.line_len(view.cursor.line));
        }
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.move-down", "Move cursor one line down", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        if view.cursor.line + 1 < buf.line_count() {
            view.cursor.line += 1;
            let target_col = view.col_memory;
            view.cursor.col = target_col.min(buf.line_len(view.cursor.line));
        }
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.line-start", "Move cursor to line start", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        view.cursor.col = 0;
        view.col_memory = 0;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.line-end", "Move cursor to line end", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        view.cursor.col = buf.line_len(view.cursor.line);
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.file-start", "Move cursor to file start", |ctx| {
        ctx.workspace.push_jump();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        view.cursor = Pos::zero();
        view.col_memory = 0;
        view.scroll_line = 0;
        view.scroll_y = 0.0;
        emit_cursor_moved(ctx, old);
    });

    reg.register("cursor.file-end", "Move cursor to file end", |ctx| {
        ctx.workspace.push_jump();
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        let last_line = buf.line_count().saturating_sub(1);
        view.cursor = Pos::new(last_line, buf.line_len(last_line));
        view.col_memory = view.cursor.col;
        emit_cursor_moved(ctx, old);
    });

    reg.register(
        "view.jump-back",
        "Jump to the previous cursor location",
        |ctx| {
            ctx.workspace.jump_back();
        },
    );

    reg.register(
        "view.jump-forward",
        "Jump to the next cursor location",
        |ctx| {
            ctx.workspace.jump_forward();
        },
    );

    reg.register("edit.goto-line", "Go to a line number", |ctx| {
        let Some(arg) = ctx.arg.clone() else {
            ctx.workspace.request_ui(UiIntent::Input {
                prompt: "go to line:".to_string(),
                command: "edit.goto-line".to_string(),
            });
            return;
        };
        let Ok(n) = arg.trim().parse::<usize>() else {
            return;
        };
        let last = ctx
            .workspace
            .buffers
            .get(&ctx.buffer_id)
            .unwrap()
            .line_count()
            .saturating_sub(1);
        let line = n.saturating_sub(1).min(last);
        ctx.workspace.push_jump();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        view.cursor = Pos::new(line, 0);
        view.col_memory = 0;
        emit_cursor_moved(ctx, old);
    });

    // --- word movement ---

    reg.register(
        "cursor.word-forward",
        "Move cursor to start of next word",
        |ctx| {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
            let pos = word_forward(buf, view.cursor);
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.clear_selection();
            view.cursor = pos;
            view.col_memory = pos.col;
            emit_cursor_moved(ctx, old);
        },
    );

    reg.register(
        "cursor.word-backward",
        "Move cursor to start of previous word",
        |ctx| {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
            let pos = word_backward(buf, view.cursor);
            let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
            let old = view.cursor;
            view.clear_selection();
            view.cursor = pos;
            view.col_memory = pos.col;
            emit_cursor_moved(ctx, old);
        },
    );

    // --- page movement ---

    reg.register("view.page-down", "Scroll down one page", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let line_count = buf.line_count();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        let page = view.page_height.max(1);
        view.cursor.line = (view.cursor.line + page).min(line_count.saturating_sub(1));
        view.cursor.col = view.cursor.col.min(buf.line_len(view.cursor.line));
        view.col_memory = view.cursor.col;
        view.scroll_line = (view.scroll_line + page).min(max_scroll_line(line_count, page));
        view.scroll_y = 0.0;
        emit_cursor_moved(ctx, old);
    });

    reg.register("view.page-up", "Scroll up one page", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let old = view.cursor;
        view.clear_selection();
        let page = view.page_height.max(1);
        view.cursor.line = view.cursor.line.saturating_sub(page);
        view.cursor.col = view.cursor.col.min(buf.line_len(view.cursor.line));
        view.col_memory = view.cursor.col;
        view.scroll_line = view.scroll_line.saturating_sub(page);
        view.scroll_y = 0.0;
        emit_cursor_moved(ctx, old);
    });

    // --- view scroll (without cursor move) ---

    reg.register("view.scroll-down", "Scroll view down one line", |ctx| {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let max = max_scroll_line(buf.line_count(), view.page_height);
        view.scroll_line = (view.scroll_line + 1).min(max);
        view.scroll_y = 0.0;
    });

    reg.register("view.scroll-up", "Scroll view up one line", |ctx| {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        view.scroll_line = view.scroll_line.saturating_sub(1);
        view.scroll_y = 0.0;
    });
}
