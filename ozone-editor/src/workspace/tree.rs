use ozone_buffer::{BufferKind, Pos};

use super::Workspace;
use crate::commands::{tree_row_dir_path, tree_row_path};
use crate::pane::{PaneTree, SplitAxis};
use crate::ui::UiIntent;
use crate::view::ViewId;

impl Workspace {
    /// The visible explorer, independent of which pane currently has focus.
    pub fn file_tree_view(&self) -> Option<ViewId> {
        self.panes.as_ref()?.leaves().into_iter().find(|id| {
            self.views
                .get(id)
                .and_then(|v| self.buffers.get(&v.buffer_id))
                .is_some_and(|b| matches!(b.kind, BufferKind::FileTree))
        })
    }

    /// Focus the explorer while remembering the editor it should open into.
    pub fn focus_file_tree(&mut self) {
        if let Some(tree) = self.file_tree_view() {
            if self.active_view_id != Some(tree) {
                self.tree_editor_view = self.active_view_id;
            }
            self.active_view_id = Some(tree);
        }
    }

    /// Refresh an existing explorer without stealing focus, or insert one to
    /// the left of the existing editor layout. Selection follows its path.
    pub fn show_file_tree(&mut self, content: String) -> ViewId {
        if let Some(id) = self.file_tree_view() {
            let view = &self.views[&id];
            let buffer = view.buffer_id;
            let old_line = view.cursor.line;
            let selected = self.buffers[&buffer].line(old_line).and_then(|line| {
                tree_row_path(&line)
                    .or_else(|| tree_row_dir_path(&line))
                    .map(str::to_owned)
            });
            let selected_line = selected.as_deref().and_then(|path| {
                content.lines().position(|line| {
                    tree_row_path(line).or_else(|| tree_row_dir_path(line)) == Some(path)
                })
            });
            self.buffers
                .get_mut(&buffer)
                .expect("visible tree buffer")
                .set_text(&content);
            let last = self.buffers[&buffer].line_count().saturating_sub(2).max(1);
            if let Some(view) = self.views.get_mut(&id) {
                view.cursor = Pos::new(selected_line.unwrap_or(old_line).min(last), 0);
                view.clear_selection();
            }
            return id;
        }
        let editors = self.panes.clone();
        self.tree_editor_view = self.active_view_id;
        let (_, tree) = self.open_virtual_buffer(BufferKind::FileTree, content);
        if let Some(editors) = editors {
            self.panes = Some(PaneTree::Split {
                axis: SplitAxis::Vertical,
                ratio: 0.23,
                first: Box::new(PaneTree::leaf(tree)),
                second: Box::new(editors),
            });
        }
        if let Some(view) = self.views.get_mut(&tree) {
            view.cursor = Pos::new(1, 0);
        }
        tree
    }

    /// Pick an editor to the right before opening a tree item, so the
    /// explorer remains visible. Returns true if a new editor pane was needed.
    pub fn focus_tree_editor(&mut self) -> bool {
        let Some(tree) = self.file_tree_view() else {
            return false;
        };
        let editor = self.panes.as_ref().and_then(|panes| {
            panes.leaves().into_iter().find(|id| {
                *id != tree
                    && self
                        .views
                        .get(id)
                        .and_then(|v| self.buffers.get(&v.buffer_id))
                        .is_some_and(|b| !matches!(b.kind, BufferKind::FileTree))
            })
        });
        let editor = self
            .tree_editor_view
            .filter(|id| self.panes.as_ref().is_some_and(|p| p.contains(*id)))
            .filter(|id| {
                self.views
                    .get(id)
                    .and_then(|v| self.buffers.get(&v.buffer_id))
                    .is_some_and(|b| !matches!(b.kind, BufferKind::FileTree))
            })
            .or(editor);
        if let Some(editor) = editor {
            self.active_view_id = Some(editor);
            false
        } else {
            self.active_view_id = Some(tree);
            self.split_active_pane(SplitAxis::Vertical).is_some()
        }
    }

    /// Explorer-style Right expands or enters a folder; Left collapses it
    /// or moves to its parent. Refreshes are queued for the GUI worker.
    pub fn navigate_file_tree(&mut self, right: bool) {
        let Some(view) = self.active_view() else {
            return;
        };
        let buffer_id = view.buffer_id;
        let line = view.cursor.line;
        let Some(buffer) = self.buffers.get(&buffer_id) else {
            return;
        };
        if !matches!(buffer.kind, BufferKind::FileTree) {
            return;
        }
        let Some(row) = buffer.line(line) else { return };
        let dir = tree_row_dir_path(&row);
        if let Some(path) = dir {
            let collapsed = self.tree_collapsed.contains(path);
            if right && collapsed || !right && !collapsed {
                if right {
                    self.tree_collapsed.remove(path);
                } else {
                    self.tree_collapsed.insert(path.to_string());
                }
                self.request_ui(UiIntent::FileTree {
                    collapsed: self.tree_collapsed.clone(),
                });
                return;
            }
        }
        let target = if right {
            dir.and_then(|path| {
                let next = buffer.line(line + 1)?;
                let next_path = tree_row_path(&next).or_else(|| tree_row_dir_path(&next))?;
                next_path
                    .starts_with(&format!("{path}/"))
                    .then_some(line + 1)
            })
        } else {
            tree_row_path(&row)
                .or(dir)
                .and_then(|path| path.rsplit_once('/'))
                .and_then(|(parent, _)| {
                    (1..line).rev().find(|&i| {
                        buffer
                            .line(i)
                            .is_some_and(|row| tree_row_dir_path(&row) == Some(parent))
                    })
                })
        };
        if let Some(line) = target
            && let Some(view) = self.active_view_mut()
        {
            view.cursor = Pos::new(line, 0);
            view.clear_selection();
            view.scroll_to_cursor(view.page_height.max(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandContext, CommandRegistry, register_defaults};

    const TREE: &str = "▸ workspace/\n├── ▾ src/\tsrc/\n│   └── main.rs\tsrc/main.rs\n└── Cargo.toml\tCargo.toml\n";

    #[test]
    fn explorer_wraps_existing_splits_and_returns_to_the_original_editor() {
        let mut ws = Workspace::new();
        let first = ws.active_view_id.expect("editor");
        let second = ws.split_active_pane(SplitAxis::Horizontal).expect("split");
        let editors = ws.panes.clone().expect("editor layout");
        let tree = ws.show_file_tree(TREE.into());
        let PaneTree::Split {
            axis,
            first: left,
            second: right,
            ..
        } = ws.panes.as_ref().expect("layout")
        else {
            panic!("explorer should be beside the editor layout");
        };
        assert_eq!(*axis, SplitAxis::Vertical);
        assert_eq!(**left, PaneTree::leaf(tree));
        assert_eq!(**right, editors);
        ws.focus_tree_editor();
        assert_eq!(ws.active_view_id, Some(second));
        ws.active_view_id = Some(first);
        ws.focus_file_tree();
        ws.focus_tree_editor();
        assert_eq!(ws.active_view_id, Some(first));
        assert!(ws.close_view(tree));
        assert_eq!(ws.panes, Some(editors));
    }

    #[test]
    fn refresh_keeps_selection_by_path_and_does_not_steal_editor_focus() {
        let mut ws = Workspace::new();
        let editor = ws.active_view_id;
        let tree = ws.show_file_tree(TREE.into());
        ws.views.get_mut(&tree).expect("tree").cursor = Pos::new(3, 0);
        ws.focus_tree_editor();
        let layout = ws.panes.clone();
        let refreshed = TREE.replace("└── Cargo.toml", "├── extra.rs\textra.rs\n└── Cargo.toml");
        assert_eq!(ws.show_file_tree(refreshed), tree);
        assert_eq!(ws.active_view_id, editor);
        assert_eq!(ws.views[&tree].cursor.line, 4);
        assert_eq!(ws.panes, layout);
        assert_eq!(
            ws.buffers
                .values()
                .filter(|b| matches!(b.kind, BufferKind::FileTree))
                .count(),
            1
        );
    }

    #[test]
    fn arrows_expand_collapse_enter_children_and_select_parent_without_scanning() {
        let mut ws = Workspace::new();
        let tree = ws.show_file_tree(TREE.into());
        ws.navigate_file_tree(true);
        assert_eq!(
            ws.views[&tree].cursor.line, 2,
            "Right enters expanded directory"
        );
        ws.navigate_file_tree(false);
        assert_eq!(ws.views[&tree].cursor.line, 1, "Left goes to parent");
        ws.navigate_file_tree(false);
        assert!(ws.tree_collapsed.contains("src"));
        assert!(matches!(
            ws.drain_ui_intents().as_slice(),
            [UiIntent::FileTree { .. }]
        ));
        ws.navigate_file_tree(true);
        assert!(!ws.tree_collapsed.contains("src"));
        assert!(matches!(
            ws.drain_ui_intents().as_slice(),
            [UiIntent::FileTree { .. }]
        ));
        assert_eq!(ws.views[&tree].cursor.line, 1);
    }

    #[test]
    fn file_tree_command_focuses_then_closes_the_same_explorer() {
        let mut ws = Workspace::new();
        let tree = ws.show_file_tree(TREE.into());
        ws.focus_tree_editor();
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        reg.execute(
            "file.tree",
            &mut CommandContext::new(&mut ws).expect("context"),
        );
        assert_eq!(ws.active_view_id, Some(tree));
        assert!(ws.drain_ui_intents().is_empty());
        reg.execute(
            "file.tree",
            &mut CommandContext::new(&mut ws).expect("context"),
        );
        assert_eq!(ws.file_tree_view(), None);
    }

    #[test]
    fn opening_a_tree_file_keeps_the_explorer_and_reuses_an_existing_buffer() {
        let path = std::env::temp_dir().join(format!("ozone-tree-open-{}.rs", std::process::id()));
        std::fs::write(&path, "fn main() {}\n").expect("fixture");
        let mut ws = Workspace::new();
        let (buffer, _) = ws.open_file(path.clone()).expect("open");
        let tree = ws.show_file_tree(format!(
            "▸ workspace/\n└── file.rs\t{}\n",
            path.to_string_lossy()
        ));
        let mut reg = CommandRegistry::new();
        register_defaults(&mut reg);
        let count = ws.buffers.len();
        reg.execute(
            "tree.open-selection",
            &mut CommandContext::new(&mut ws).expect("context"),
        );
        assert_eq!(ws.file_tree_view(), Some(tree));
        assert_ne!(ws.active_view_id, Some(tree));
        assert_eq!(ws.active_buffer().map(|b| b.id), Some(buffer));
        assert_eq!(ws.buffers.len(), count);
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn row_metadata_keeps_repeated_spaces_in_file_and_directory_names() {
        assert_eq!(
            tree_row_path("└── my  file.rs\tmy  dir/my  file.rs"),
            Some("my  dir/my  file.rs")
        );
        assert_eq!(
            tree_row_dir_path("├── ▾ my  dir/\tmy  dir/"),
            Some("my  dir")
        );
    }
}
