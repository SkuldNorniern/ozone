//! Fold commands: toggle the fold at the cursor, fold every region, or open
//! all. Fold state is view-local (`View::folds`); see [`crate::fold`].

use crate::fold;

use super::CommandRegistry;

pub(super) fn register_fold_commands(reg: &mut CommandRegistry) {
    reg.register("fold.toggle", "Toggle the fold at the cursor", |ctx| {
        let Some(buf) = ctx.workspace.buffers.get(&ctx.buffer_id) else {
            return;
        };
        let cursor_line = ctx
            .workspace
            .views
            .get(&ctx.view_id)
            .map(|v| v.cursor.line)
            .unwrap_or(0);
        let Some(header) = fold::header_for(buf, cursor_line) else {
            return;
        };
        if let Some(view) = ctx.workspace.views.get_mut(&ctx.view_id)
            && !view.folds.remove(&header)
        {
            view.folds.insert(header);
            // Keep the cursor on the visible header line.
            view.cursor.line = header;
            view.cursor.col = view.cursor.col.min(buf.line_len(header));
            view.col_memory = view.cursor.col;
        }
    });

    reg.register("fold.all", "Fold every region in the buffer", |ctx| {
        let Some(buf) = ctx.workspace.buffers.get(&ctx.buffer_id) else {
            return;
        };
        let headers = fold::all_headers(buf);
        if let Some(view) = ctx.workspace.views.get_mut(&ctx.view_id) {
            view.folds.extend(headers);
        }
    });

    reg.register("fold.open-all", "Open all folds in the buffer", |ctx| {
        if let Some(view) = ctx.workspace.views.get_mut(&ctx.view_id) {
            view.folds.clear();
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::commands::register_defaults;
    use crate::workspace::Workspace;
    use crate::{CommandContext, CommandRegistry};
    use ozone_buffer::Pos;

    fn ws_with(text: &str, cursor_line: usize) -> Workspace {
        let mut ws = Workspace::new();
        ws.active_buffer_mut().unwrap().set_text(text);
        ws.active_view_mut().unwrap().cursor = Pos::new(cursor_line, 0);
        ws
    }

    fn run(ws: &mut Workspace, command: &str) {
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let mut ctx = CommandContext::new(ws).unwrap();
        assert!(reg.execute(command, &mut ctx));
    }

    #[test]
    fn toggle_folds_and_unfolds_header() {
        let mut ws = ws_with("fn x() {\n    a;\n    b;\n}", 0);
        run(&mut ws, "fold.toggle");
        assert!(ws.active_view().unwrap().folds.contains(&0));
        run(&mut ws, "fold.toggle");
        assert!(ws.active_view().unwrap().folds.is_empty());
    }

    #[test]
    fn toggle_from_inside_uses_enclosing_header() {
        let mut ws = ws_with("root:\n  a\n  b\ntail", 2);
        run(&mut ws, "fold.toggle");
        assert!(ws.active_view().unwrap().folds.contains(&0));
        // cursor moved onto the visible header.
        assert_eq!(ws.active_view().unwrap().cursor.line, 0);
    }

    #[test]
    fn fold_all_then_open_all() {
        let mut ws = ws_with("a:\n  b\nc:\n  d", 0);
        run(&mut ws, "fold.all");
        assert_eq!(ws.active_view().unwrap().folds.len(), 2);
        run(&mut ws, "fold.open-all");
        assert!(ws.active_view().unwrap().folds.is_empty());
    }
}
