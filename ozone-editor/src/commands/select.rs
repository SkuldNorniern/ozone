//! Selection commands built on [`crate::text_object`]: select the word, line,
//! enclosing bracket pair (inside/around), or surrounding quotes at the cursor.
//! Each sets the active view's selection and moves the cursor to its end.

use ozone_buffer::{Buffer, BufferKind, Pos, Span};
use taste::detect_language;

use crate::text_object;

use super::{CommandContext, CommandRegistry, emit_cursor_moved, word_backward, word_forward};

pub(super) fn register_select_commands(reg: &mut CommandRegistry) {
    reg.register("select.word", "Select the word at the cursor", |ctx| {
        let span = with_cursor(ctx, text_object::word_at);
        apply(ctx, span);
    });
    reg.register("select.line", "Select the current line", |ctx| {
        let span = with_cursor(ctx, |buf, pos| Some(text_object::line_inner(buf, pos)));
        apply(ctx, span);
    });
    reg.register(
        "select.inside-brackets",
        "Select inside the enclosing brackets",
        |ctx| {
            let span = with_cursor(ctx, text_object::inside_brackets);
            apply(ctx, span);
        },
    );
    reg.register(
        "select.around-brackets",
        "Select the enclosing brackets and their contents",
        |ctx| {
            let span = with_cursor(ctx, text_object::around_brackets);
            apply(ctx, span);
        },
    );
    reg.register(
        "select.inside-quotes",
        "Select inside the surrounding quotes",
        |ctx| {
            let span = with_cursor(ctx, text_object::inside_quotes);
            apply(ctx, span);
        },
    );
    reg.register(
        "select.expand",
        "Expand selection to the next structural boundary",
        |ctx| {
            let span = compute_expand(ctx);
            apply(ctx, span);
        },
    );

    // ── Keyboard extend-selection (shift+arrow family) ────────────────────

    reg.register(
        "select.extend-left",
        "Extend selection one character left",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                let mut pos = view.cursor;
                if pos.col > 0 {
                    pos.col -= 1;
                } else if pos.line > 0 {
                    pos.line -= 1;
                    pos.col = buf.line_len(pos.line);
                }
                pos
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-right",
        "Extend selection one character right",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                let mut pos = view.cursor;
                let line_len = buf.line_len(pos.line);
                if pos.col < line_len {
                    pos.col += 1;
                } else if pos.line + 1 < buf.line_count() {
                    pos.line += 1;
                    pos.col = 0;
                }
                pos
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register("select.extend-up", "Extend selection one line up", |ctx| {
        let new_pos = {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
            let mut pos = view.cursor;
            if pos.line > 0 {
                pos.line -= 1;
                pos.col = view.col_memory.min(buf.line_len(pos.line));
            }
            pos
        };
        extend_to(ctx, new_pos);
    });

    reg.register(
        "select.extend-down",
        "Extend selection one line down",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                let mut pos = view.cursor;
                if pos.line + 1 < buf.line_count() {
                    pos.line += 1;
                    pos.col = view.col_memory.min(buf.line_len(pos.line));
                }
                pos
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-line-start",
        "Extend selection to the start of the line",
        |ctx| {
            let new_pos = {
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                Pos::new(view.cursor.line, 0)
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-line-end",
        "Extend selection to the end of the line",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                Pos::new(view.cursor.line, buf.line_len(view.cursor.line))
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-word-forward",
        "Extend selection to the start of the next word",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                word_forward(buf, view.cursor)
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-word-backward",
        "Extend selection to the start of the previous word",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                word_backward(buf, view.cursor)
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-file-start",
        "Extend selection to the start of the file",
        |ctx| {
            extend_to(ctx, Pos::zero());
        },
    );

    reg.register(
        "select.extend-file-end",
        "Extend selection to the end of the file",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let last = buf.line_count().saturating_sub(1);
                Pos::new(last, buf.line_len(last))
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-page-up",
        "Extend selection one page up",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                let page = view.page_height.max(1);
                let line = view.cursor.line.saturating_sub(page);
                Pos::new(line, view.col_memory.min(buf.line_len(line)))
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register(
        "select.extend-page-down",
        "Extend selection one page down",
        |ctx| {
            let new_pos = {
                let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
                let view = ctx.workspace.views.get(&ctx.view_id).unwrap();
                let page = view.page_height.max(1);
                let line = (view.cursor.line + page).min(buf.line_count().saturating_sub(1));
                Pos::new(line, view.col_memory.min(buf.line_len(line)))
            };
            extend_to(ctx, new_pos);
        },
    );

    reg.register("select.all", "Select the entire buffer", |ctx| {
        let last_pos = {
            let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
            let last = buf.line_count().saturating_sub(1);
            Pos::new(last, buf.line_len(last))
        };
        let span = Span {
            start: Pos::zero(),
            end: last_pos,
        };
        apply(ctx, Some(span));
    });
}

fn compute_expand(ctx: &CommandContext) -> Option<Span> {
    let buf = ctx.workspace.buffers.get(&ctx.buffer_id)?;
    let view = ctx.workspace.views.get(&ctx.view_id)?;
    let path = match &buf.kind {
        BufferKind::File(p) => p.to_string_lossy().into_owned(),
        _ => return None,
    };
    let lang = detect_language(&path);
    let text = buf.text();
    let (sel_start, sel_end) = match view.selection {
        Some(span) => (buf.pos_to_offset(span.start), buf.pos_to_offset(span.end)),
        None => {
            let off = buf.pos_to_offset(view.cursor);
            (off, off)
        }
    };
    let (new_start, new_end) = ozone_syntax::expand_selection(lang, &text, sel_start, sel_end)?;
    Some(Span {
        start: buf.offset_to_pos(new_start),
        end: buf.offset_to_pos(new_end),
    })
}

/// Run a text-object function against the active buffer at the cursor.
fn with_cursor(ctx: &CommandContext, f: impl FnOnce(&Buffer, Pos) -> Option<Span>) -> Option<Span> {
    let buf = ctx.workspace.buffers.get(&ctx.buffer_id)?;
    let cursor = ctx.workspace.views.get(&ctx.view_id)?.cursor;
    f(buf, cursor)
}

/// Set the view's selection to `span`, put cursor at span.end, anchor at span.start.
fn apply(ctx: &mut CommandContext, span: Option<Span>) {
    let Some(span) = span else {
        return;
    };
    let Some(view) = ctx.workspace.views.get_mut(&ctx.view_id) else {
        return;
    };
    let old = view.cursor;
    view.selection = Some(span);
    view.anchor = Some(span.start);
    view.cursor = span.end;
    view.col_memory = span.end.col;
    emit_cursor_moved(ctx, old);
}

/// Move the selection's active end (cursor) to `new_pos`, keeping the anchor fixed.
/// On first call with no existing selection, the current cursor becomes the anchor.
fn extend_to(ctx: &mut CommandContext, new_pos: Pos) {
    let (anchor, old) = {
        let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
        let a = *view.anchor.get_or_insert(view.cursor);
        (a, view.cursor)
    };
    let (anchor_off, new_off) = {
        let buf = ctx.workspace.buffers.get(&ctx.buffer_id).unwrap();
        (buf.pos_to_offset(anchor), buf.pos_to_offset(new_pos))
    };
    let view = ctx.workspace.views.get_mut(&ctx.view_id).unwrap();
    view.cursor = new_pos;
    view.col_memory = new_pos.col;
    if new_off == anchor_off {
        view.selection = None;
        view.anchor = None;
    } else {
        let (start, end) = if new_off < anchor_off {
            (new_pos, anchor)
        } else {
            (anchor, new_pos)
        };
        view.selection = Some(Span { start, end });
    }
    emit_cursor_moved(ctx, old);
}

#[cfg(test)]
mod tests {
    use crate::commands::register_defaults;
    use crate::workspace::Workspace;
    use crate::{CommandContext, CommandRegistry};
    use ozone_buffer::{Pos, Span};

    fn run(text: &str, cursor: Pos, command: &str) -> (Span, Pos) {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text(text);
        ws.active_view_mut().unwrap().cursor = cursor;
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let mut ctx = CommandContext::new(&mut ws).unwrap();
        assert!(reg.execute(command, &mut ctx));
        let view = ws.active_view().unwrap();
        (view.selection.unwrap(), view.cursor)
    }

    #[test]
    fn select_word_sets_selection_and_cursor() {
        let (sel, cursor) = run("foo bar baz", Pos::new(0, 5), "select.word");
        assert_eq!(sel.start, Pos::new(0, 4));
        assert_eq!(sel.end, Pos::new(0, 7));
        assert_eq!(cursor, Pos::new(0, 7));
    }

    #[test]
    fn select_inside_brackets_command() {
        let (sel, _) = run("call(a, b)", Pos::new(0, 6), "select.inside-brackets");
        assert_eq!(sel.start, Pos::new(0, 5));
        assert_eq!(sel.end, Pos::new(0, 9));
    }

    #[test]
    fn select_line_command() {
        let (sel, cursor) = run("hello\nworld", Pos::new(0, 2), "select.line");
        assert_eq!(sel.start, Pos::new(0, 0));
        assert_eq!(sel.end, Pos::new(0, 5));
        assert_eq!(cursor, Pos::new(0, 5));
    }

    fn run_extend(text: &str, cursor: Pos, commands: &[&str]) -> (Option<Span>, Pos) {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text(text);
        ws.active_view_mut().unwrap().cursor = cursor;
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        for &cmd in commands {
            let mut ctx = CommandContext::new(&mut ws).unwrap();
            reg.execute(cmd, &mut ctx);
        }
        let view = ws.active_view().unwrap();
        (view.selection, view.cursor)
    }

    #[test]
    fn extend_right_creates_selection() {
        let (sel, cursor) = run_extend("hello", Pos::new(0, 1), &["select.extend-right"]);
        assert_eq!(
            sel,
            Some(Span {
                start: Pos::new(0, 1),
                end: Pos::new(0, 2)
            })
        );
        assert_eq!(cursor, Pos::new(0, 2));
    }

    #[test]
    fn extend_left_creates_selection_backward() {
        let (sel, cursor) = run_extend("hello", Pos::new(0, 3), &["select.extend-left"]);
        assert_eq!(
            sel,
            Some(Span {
                start: Pos::new(0, 2),
                end: Pos::new(0, 3)
            })
        );
        // cursor tracks the active (moving) end — leftward, so cursor is at start
        assert_eq!(cursor, Pos::new(0, 2));
    }

    #[test]
    fn extend_then_retract_clears_selection() {
        let (sel, cursor) = run_extend(
            "hello",
            Pos::new(0, 2),
            &["select.extend-right", "select.extend-left"],
        );
        assert_eq!(sel, None);
        assert_eq!(cursor, Pos::new(0, 2));
    }

    #[test]
    fn extend_right_twice_grows_selection() {
        let (sel, cursor) = run_extend(
            "hello",
            Pos::new(0, 1),
            &["select.extend-right", "select.extend-right"],
        );
        assert_eq!(
            sel,
            Some(Span {
                start: Pos::new(0, 1),
                end: Pos::new(0, 3)
            })
        );
        assert_eq!(cursor, Pos::new(0, 3));
    }

    #[test]
    fn cursor_move_after_extend_clears_selection() {
        let (sel, cursor) = run_extend(
            "hello",
            Pos::new(0, 1),
            &["select.extend-right", "cursor.move-right"],
        );
        assert_eq!(sel, None);
        assert_eq!(cursor, Pos::new(0, 3));
    }

    #[test]
    fn select_all_covers_whole_buffer() {
        let (sel, cursor) = run_extend("hi\nworld", Pos::new(0, 0), &["select.all"]);
        assert_eq!(
            sel,
            Some(Span {
                start: Pos::new(0, 0),
                end: Pos::new(1, 5)
            })
        );
        assert_eq!(cursor, Pos::new(1, 5));
    }
}
