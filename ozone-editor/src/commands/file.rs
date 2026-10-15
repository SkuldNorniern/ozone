use std::env;

use ozone_buffer::{BufferKind, Pos};

use crate::ui::{NotifyLevel, UiIntent};
use crate::workspace_search::WorkspaceMatch;

use super::{CommandRegistry, buffer_display_name, tree_row_dir_path, tree_row_path};

pub(super) fn register_file_commands(reg: &mut CommandRegistry) {
    // --- file ---

    reg.register("file.save", "Save the current buffer", |ctx| {
        let id = ctx.buffer_id;
        let name = buffer_display_name(ctx.workspace, id);
        match ctx.workspace.save_buffer(id) {
            Ok(()) => ctx
                .workspace
                .notify(NotifyLevel::Success, format!("Saved {name}")),
            Err(e) => ctx
                .workspace
                .notify(NotifyLevel::Error, format!("Save failed: {e}")),
        }
    });

    reg.register("file.save-all", "Save all dirty buffers", |ctx| {
        let ids: Vec<_> = ctx.workspace.buffers.keys().copied().collect();
        let mut saved = 0usize;
        for id in ids {
            match ctx.workspace.save_buffer(id) {
                Ok(()) => saved += 1,
                Err(e) => ctx
                    .workspace
                    .notify(NotifyLevel::Error, format!("Save failed: {e}")),
            }
        }
        ctx.workspace
            .notify(NotifyLevel::Success, format!("Saved {saved} buffer(s)"));
    });

    reg.register(
        "config.reload",
        "Reload config from disk (keymaps, modifiers, autocommands)",
        |ctx| {
            ctx.workspace.request_config_reload();
        },
    );

    // Frontend-driven overlays: real commands that queue a UiIntent for the GUI.
    reg.register("command.palette", "Open the command palette", |ctx| {
        ctx.workspace.request_ui(UiIntent::CommandPalette);
    });
    reg.register("file.picker", "Open the workspace file picker", |ctx| {
        ctx.workspace.request_ui(UiIntent::FilePicker);
    });
    reg.register(
        "file.tree",
        "Show, focus or close the workspace explorer",
        |ctx| {
            if let Some(tree) = ctx.workspace.file_tree_view() {
                if ctx.workspace.active_view_id == Some(tree) {
                    ctx.workspace.close_view(tree);
                } else {
                    ctx.workspace.focus_file_tree();
                }
                return;
            }
            let collapsed = ctx.workspace.tree_collapsed.clone();
            ctx.workspace.request_ui(UiIntent::FileTree { collapsed });
        },
    );
    reg.register("tree.refresh", "Refresh the workspace explorer", |ctx| {
        ctx.workspace.request_ui(UiIntent::FileTree {
            collapsed: ctx.workspace.tree_collapsed.clone(),
        });
    });
    reg.register(
        "tree.expand-selection",
        "Expand directory or select its first child",
        |ctx| {
            ctx.workspace.navigate_file_tree(true);
        },
    );
    reg.register(
        "tree.collapse-selection",
        "Collapse directory or select its parent",
        |ctx| {
            ctx.workspace.navigate_file_tree(false);
        },
    );
    reg.register(
        "file.open-folder",
        "Open a folder as the workspace via the native OS folder picker",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::OpenFolderPicker);
        },
    );
    reg.register(
        "file.open-file",
        "Open a file via the native OS file picker",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::OpenFilePicker);
        },
    );
    reg.register(
        "buffer.picker",
        "Switch to an open buffer (fuzzy picker)",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::BufferPicker);
        },
    );
    reg.register("theme.select", "Select an installed color theme", |ctx| {
        ctx.workspace.request_ui(UiIntent::ThemePicker);
    });
    reg.register(
        "symbol.picker",
        "Jump to a symbol in the current buffer",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::SymbolPicker);
        },
    );
    reg.register("search.start", "Incremental search in the buffer", |ctx| {
        ctx.workspace.request_ui(UiIntent::SearchStart);
    });
    reg.register(
        "search.replace",
        "Search and replace in the buffer",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::SearchReplace);
        },
    );
    reg.register(
        "search.workspace",
        "Search for text across workspace files",
        |ctx| {
            let Some(query) = ctx.arg.clone() else {
                ctx.workspace.request_ui(UiIntent::Input {
                    prompt: "workspace search:".to_string(),
                    command: "search.workspace".to_string(),
                });
                return;
            };
            let query = query.trim();
            if query.is_empty() {
                return;
            }
            ctx.workspace.request_ui(UiIntent::WorkspaceSearch {
                query: query.to_string(),
            });
        },
    );

    reg.register(
        "picker.open-selection",
        "Open the file on the picker's current line",
        |ctx| {
            let is_picker = matches!(
                ctx.workspace.buffers.get(&ctx.buffer_id).map(|b| &b.kind),
                Some(BufferKind::Search)
            );
            if !is_picker {
                return;
            }
            let line = ctx.workspace.views.get(&ctx.view_id).and_then(|view| {
                ctx.workspace
                    .buffers
                    .get(&ctx.buffer_id)
                    .and_then(|buf| buf.line(view.cursor.line))
            });
            let Some(line) = line else { return };
            let rel = line.trim();
            if rel.is_empty() {
                return;
            }
            let Ok(base) = env::current_dir() else {
                return;
            };
            let target = base.join(rel);
            if !target.is_file() {
                return;
            }
            let picker_view = ctx.view_id;
            if ctx.workspace.open_file(target).is_ok() {
                ctx.workspace.discard_orphan_view(picker_view);
            }
        },
    );

    reg.register(
        "references.open-selection",
        "Open the selected workspace search result",
        |ctx| {
            let is_references = matches!(
                ctx.workspace.buffers.get(&ctx.buffer_id).map(|b| &b.kind),
                Some(BufferKind::References)
            );
            if !is_references {
                return;
            }
            let row = ctx.workspace.views.get(&ctx.view_id).and_then(|view| {
                ctx.workspace
                    .buffers
                    .get(&ctx.buffer_id)
                    .and_then(|buf| buf.line(view.cursor.line))
            });
            let Some(hit) = row.as_deref().and_then(WorkspaceMatch::parse) else {
                return;
            };
            let Ok(base) = env::current_dir() else {
                return;
            };
            let references_view = ctx.view_id;
            let Ok((buffer_id, view_id)) = ctx.workspace.open_file(base.join(&hit.path)) else {
                return;
            };
            let (line, column) = {
                let buffer = ctx.workspace.buffers.get(&buffer_id).unwrap();
                let line = hit.line.min(buffer.line_count().saturating_sub(1));
                (line, hit.column.min(buffer.line_len(line)))
            };
            if let Some(view) = ctx.workspace.views.get_mut(&view_id) {
                view.cursor = Pos::new(line, column);
                view.col_memory = column;
                view.scroll_to_cursor(view.page_height.max(1));
            }
            ctx.workspace.discard_orphan_view(references_view);
        },
    );

    reg.register(
        "tree.open-selection",
        "Open file or toggle directory collapse in the file tree",
        |ctx| {
            let is_tree = matches!(
                ctx.workspace.buffers.get(&ctx.buffer_id).map(|b| &b.kind),
                Some(BufferKind::FileTree)
            );
            if !is_tree {
                return;
            }
            let row = ctx.workspace.views.get(&ctx.view_id).and_then(|view| {
                ctx.workspace
                    .buffers
                    .get(&ctx.buffer_id)
                    .and_then(|buf| buf.line(view.cursor.line))
            });
            let Some(row) = row else { return };

            // Dir row: toggle collapse and refresh the tree in-place.
            if let Some(dir_path) = tree_row_dir_path(&row) {
                let dir_path = dir_path.to_string();
                if ctx.workspace.tree_collapsed.contains(&dir_path) {
                    ctx.workspace.tree_collapsed.remove(&dir_path);
                } else {
                    ctx.workspace.tree_collapsed.insert(dir_path);
                }
                ctx.workspace.request_ui(UiIntent::FileTree {
                    collapsed: ctx.workspace.tree_collapsed.clone(),
                });
                return;
            }

            // File row: open the file.
            let Some(path) = tree_row_path(&row) else {
                return;
            };
            let Ok(base) = env::current_dir() else {
                return;
            };
            let target = base.join(path);
            if target.is_file() {
                let previous = ctx.workspace.active_view_id;
                let created_editor = ctx.workspace.focus_tree_editor();
                let temporary_editor = ctx.workspace.active_view_id.filter(|_| created_editor);
                let target = target.canonicalize().unwrap_or(target);
                let existing = ctx
                    .workspace
                    .buffers
                    .iter()
                    .find_map(|(id, buffer)| match &buffer.kind {
                        BufferKind::File(path) | BufferKind::Image(path) if path == &target => {
                            Some(*id)
                        }
                        _ => None,
                    });
                if let Some(id) = existing {
                    ctx.workspace.switch_active_buffer(id);
                } else if let Err(error) = ctx.workspace.open_file(target) {
                    if let Some(editor) = temporary_editor {
                        ctx.workspace.close_view(editor);
                    }
                    ctx.workspace.active_view_id = previous;
                    ctx.workspace
                        .notify(NotifyLevel::Error, format!("Cannot open file: {error}"));
                }
                if let Some(editor) = temporary_editor {
                    ctx.workspace.discard_orphan_view(editor);
                }
            }
        },
    );

    reg.register(
        "lsp.goto-definition",
        "Jump to the definition of the symbol under the cursor",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::LspGotoDefinition);
        },
    );

    reg.register(
        "lsp.hover",
        "Show hover documentation for the symbol under the cursor",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::LspHover);
        },
    );

    reg.register(
        "lsp.completion",
        "Show completions for the symbol under the cursor",
        |ctx| {
            ctx.workspace.request_ui(UiIntent::LspCompletion);
        },
    );

    reg.register(
        "terminal.open",
        "Open a terminal buffer placeholder",
        |ctx| {
            ctx.workspace.open_virtual_buffer(
                BufferKind::Terminal,
                "Terminal\n--------\nProcess-backed terminal buffers are planned next.\n"
                    .to_string(),
            );
        },
    );
}
