//! Selection commands built on [`crate::text_object`]: select the word, line,
//! enclosing bracket pair (inside/around), or surrounding quotes at the cursor.
//! Each sets the active view's selection and moves the cursor to its end.

use ozone_buffer::{Buffer, BufferKind, Pos, Span};
use taste::detect_language;

use crate::text_object;

use super::{CommandContext, CommandRegistry, emit_cursor_moved};

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

/// Set the view's selection to `span` and put the cursor at its end.
fn apply(ctx: &mut CommandContext, span: Option<Span>) {
    let Some(span) = span else {
        return;
    };
    let Some(view) = ctx.workspace.views.get_mut(&ctx.view_id) else {
        return;
    };
    let old = view.cursor;
    view.selection = Some(span);
    view.cursor = span.end;
    view.col_memory = span.end.col;
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
}
