pub mod autocmd;
pub mod brackets;
pub mod commands;
pub mod events;
pub mod keymap;
pub mod pane;
pub mod search;
pub mod view;
pub mod workspace;

pub use autocmd::{Autocommand, AutocommandRegistry};
pub use brackets::matching_bracket;
pub use commands::{CommandContext, CommandRegistry};
pub use events::{EditorEvent, EventKind};
pub use keymap::{Key, KeyStroke, Keymap, KeymapOutcome, ModifierMap, PhysicalModifier, PhysicalMods};
pub use pane::{PaneTree, SplitAxis};
pub use search::find_matches;
pub use view::{View, ViewId};
pub use workspace::{IndentConfig, Workspace};
