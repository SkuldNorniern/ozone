pub mod api;
pub mod autocmd;
pub mod brackets;
pub mod commands;
pub mod decoration;
pub mod events;
pub mod keymap;
pub mod options;
pub mod pane;
pub mod search;
pub mod ui;
pub mod view;
pub mod workspace;
pub mod workspace_search;

pub use api::EditorApi;
pub use autocmd::{Autocommand, AutocommandRegistry};
pub use brackets::matching_bracket;
pub use commands::{CommandContext, CommandRegistry};
pub use decoration::{
    BRACKET_NAMESPACE, Decoration, DecorationId, DecorationKind, DecorationStore, Gravity, HlRole,
    NamespaceId, VirtualPos,
};
pub use events::{EditorEvent, EventKind};
pub use keymap::{
    Key, KeyStroke, Keymap, KeymapOutcome, ModifierMap, PhysicalModifier, PhysicalMods,
    stroke_label,
};
pub use options::{BufferLocal, OptionValue};
pub use pane::{FocusDirection, PaneTree, SplitAxis};
pub use search::find_matches;
pub use ui::{NotifyLevel, SelectItem, UiIntent};
pub use view::{View, ViewId};
pub use workspace::{IndentConfig, Workspace};
pub use workspace_search::{WorkspaceMatch, search_workspace};
