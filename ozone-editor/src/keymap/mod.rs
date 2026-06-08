//! Layered keymap with chord support.
//!
//! A keymap resolves a sequence of [`KeyStroke`]s to a command name. Bindings
//! live in layers (lowest priority first): shipped **defaults**, then user
//! **global** config, then **filetype**-scoped config. Within a resolution the
//! highest-priority binding wins.
//!
//! Chords like `ctrl+k ctrl+s` are multi-stroke sequences. Resolution is fed one
//! stroke at a time together with the strokes already pending; it returns
//! [`KeymapOutcome::Pending`] while a longer binding could still match, so the
//! caller holds the pending prefix between key events.

use ozone_config::KeymapConfig;

mod keys;
mod stroke;

pub use keys::{ModifierMap, PhysicalModifier, PhysicalMods};
pub use stroke::{Key, KeyStroke, chord_label, parse_chord, stroke_label};

/// Priority layer a binding belongs to (higher wins on exact-match ties).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    Default = 0,
    Global = 1,
    Filetype = 2,
}

#[derive(Debug, Clone)]
struct Binding {
    chord: Vec<KeyStroke>,
    command: String,
    filetype: Option<String>,
    layer: Layer,
}

/// The result of feeding a stroke to the keymap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapOutcome {
    /// A binding matched; run this command and clear the pending prefix.
    Execute(String),
    /// The current prefix could still grow into a binding; keep it pending.
    Pending,
    /// Nothing matches; clear the pending prefix.
    NoMatch,
}

/// A layered keymap. Build defaults, then layer user config on top.
#[derive(Debug, Default)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// The shipped default layer.
    pub fn with_defaults() -> Self {
        let mut km = Self::new();
        let defaults: &[(&str, &str)] = &[
            // ── File / edit (Ctrl = universal editor key) ──────────────────────
            ("ctrl+s", "file.save"),
            ("ctrl+z", "edit.undo"),
            ("ctrl+y", "edit.redo"),
            ("ctrl+p", "file.picker"),
            ("ctrl+shift+e", "file.tree"),
            ("meta+x", "command.palette"),
            ("ctrl+shift+p", "command.palette"),
            ("ctrl+tab", "buffer.next"),
            ("ctrl+shift+tab", "buffer.previous"),
            ("ctrl+x b", "buffer.picker"),
            ("meta+left", "view.jump-back"),
            ("meta+right", "view.jump-forward"),
            ("ctrl+-", "view.jump-back"),
            ("ctrl+=", "view.jump-forward"),
            ("ctrl+k ctrl+s", "file.save-all"),
            // ── Super+ aliases (Cmd on macOS, Win on Linux) ────────────────────
            ("super+s", "file.save"),
            ("super+z", "edit.undo"),
            ("super+shift+z", "edit.redo"),
            ("super+p", "file.picker"),
            ("super+shift+p", "command.palette"),
            ("super+shift+f", "search.workspace"),
            ("super+shift+e", "file.tree"),
            ("super+tab", "buffer.next"),
            ("super+shift+tab", "buffer.previous"),
            // ── Panes ─────────────────────────────────────────────────────────
            ("ctrl+shift+right", "pane.split-right"),
            ("ctrl+shift+down", "pane.split-down"),
            ("ctrl+shift+w", "pane.close"),
            ("ctrl+meta+right", "pane.focus-right"),
            ("ctrl+meta+left", "pane.focus-left"),
            ("ctrl+meta+down", "pane.focus-down"),
            ("ctrl+meta+up", "pane.focus-up"),
            // ── Emacs-style movement (Ctrl) ────────────────────────────────────
            ("ctrl+a", "cursor.line-start"),
            ("ctrl+e", "cursor.line-end"),
            ("ctrl+b", "cursor.move-left"),
            ("ctrl+f", "cursor.move-right"),
            ("ctrl+n", "cursor.move-down"),
            ("meta+f", "search.start"),
            ("meta+h", "search.replace"),
            ("ctrl+shift+f", "search.workspace"),
            ("meta+g", "edit.goto-line"),
            ("ctrl+home", "cursor.file-start"),
            ("ctrl+end", "cursor.file-end"),
            ("ctrl+left", "cursor.word-backward"),
            ("ctrl+right", "cursor.word-forward"),
            // ── Plain navigation ──────────────────────────────────────────────
            ("up", "cursor.move-up"),
            ("down", "cursor.move-down"),
            ("left", "cursor.move-left"),
            ("right", "cursor.move-right"),
            ("home", "cursor.line-start"),
            ("end", "cursor.line-end"),
            ("pageup", "view.page-up"),
            ("pagedown", "view.page-down"),
            // ── Editing ───────────────────────────────────────────────────────
            ("backspace", "edit.delete-char-backward"),
            ("delete", "edit.delete-char-forward"),
            ("enter", "edit.insert-newline"),
        ];
        for (keys, cmd) in defaults {
            km.bind_default(keys, cmd);
        }
        km
    }

    /// Add a shipped default binding (lowest priority).
    pub fn bind_default(&mut self, keys: &str, command: &str) {
        if let Some(chord) = parse_chord(keys) {
            self.bindings.push(Binding {
                chord,
                command: command.to_string(),
                filetype: None,
                layer: Layer::Default,
            });
        }
    }

    /// Layer user `[[keymap]]` config on top of the defaults.
    pub fn add_user_config(&mut self, configs: &[KeymapConfig]) {
        for cfg in configs {
            let Some(chord) = parse_chord(&cfg.keys) else {
                continue;
            };
            let layer = if cfg.filetype.is_some() {
                Layer::Filetype
            } else {
                Layer::Global
            };
            self.bindings.push(Binding {
                chord,
                command: cfg.command.clone(),
                filetype: cfg.filetype.clone(),
                layer,
            });
        }
    }

    fn applies(binding: &Binding, filetype: Option<&str>) -> bool {
        match &binding.filetype {
            None => true,
            Some(ft) => filetype == Some(ft.as_str()),
        }
    }

    /// Possible continuations of a pending chord prefix, for a which-key popup.
    pub fn continuations(
        &self,
        pending: &[KeyStroke],
        filetype: Option<&str>,
    ) -> Vec<(String, String)> {
        use std::collections::BTreeMap;
        let mut next: BTreeMap<String, (Layer, String)> = BTreeMap::new();
        for b in &self.bindings {
            if !Self::applies(b, filetype) || b.chord.len() <= pending.len() {
                continue;
            }
            if b.chord[..pending.len()] != *pending {
                continue;
            }
            let stroke = &b.chord[pending.len()];
            let label = stroke_label(stroke);
            let desc = if b.chord.len() == pending.len() + 1 {
                b.command.clone()
            } else {
                "+prefix".to_string()
            };
            next.entry(label)
                .and_modify(|(layer, d)| {
                    if b.layer > *layer || (*d == "+prefix" && desc != "+prefix") {
                        *layer = b.layer;
                        *d = desc.clone();
                    }
                })
                .or_insert((b.layer, desc));
        }
        next.into_iter().map(|(k, (_, d))| (k, d)).collect()
    }

    /// A stable sample of active bindings for help/welcome surfaces.
    pub fn display_bindings(&self, filetype: Option<&str>, limit: usize) -> Vec<(String, String)> {
        let mut rows: Vec<(Vec<KeyStroke>, Layer, String)> = Vec::new();
        for binding in &self.bindings {
            if !Self::applies(binding, filetype) {
                continue;
            }
            if let Some((_, layer, command)) = rows
                .iter_mut()
                .find(|(chord, _, _)| chord.as_slice() == binding.chord.as_slice())
            {
                if binding.layer >= *layer {
                    *layer = binding.layer;
                    *command = binding.command.clone();
                }
                continue;
            }
            rows.push((
                binding.chord.clone(),
                binding.layer,
                binding.command.clone(),
            ));
        }
        rows.into_iter()
            .take(limit)
            .map(|(chord, _, command)| (chord_label(&chord), command))
            .collect()
    }

    /// Resolve `pending + stroke` against the keymap for the active filetype.
    pub fn resolve(
        &self,
        pending: &[KeyStroke],
        stroke: &KeyStroke,
        filetype: Option<&str>,
    ) -> KeymapOutcome {
        let mut seq: Vec<KeyStroke> = pending.to_vec();
        seq.push(stroke.clone());

        let has_longer = self.bindings.iter().any(|b| {
            Self::applies(b, filetype)
                && b.chord.len() > seq.len()
                && b.chord[..seq.len()] == seq[..]
        });
        if has_longer {
            return KeymapOutcome::Pending;
        }

        let best = self
            .bindings
            .iter()
            .filter(|b| Self::applies(b, filetype) && b.chord == seq)
            .max_by_key(|b| b.layer);
        match best {
            Some(b) => KeymapOutcome::Execute(b.command.clone()),
            None => KeymapOutcome::NoMatch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(c: char) -> KeyStroke {
        KeyStroke::key(Key::Char(c))
    }

    #[test]
    fn parses_modifiers_and_key() {
        let k = KeyStroke::parse("ctrl+shift+f").unwrap();
        assert!(k.control && k.shift && !k.meta && !k.super_);
        assert_eq!(k.key, Key::Char('f'));
        assert!(KeyStroke::parse("ctrl").is_none());
        assert_eq!(KeyStroke::parse("ctrl+k ctrl+s").is_none(), false);
        assert_eq!(Key::parse("f5"), Some(Key::F(5)));
        assert_eq!(Key::parse("pgdn"), Some(Key::PageDown));
        assert_eq!(KeyStroke::parse("option+x"), KeyStroke::parse("meta+x"));
        assert!(KeyStroke::parse("super+p").unwrap().super_);
    }

    #[test]
    fn modifier_map_resolves_logical_from_physical() {
        let map = ModifierMap {
            control: PhysicalModifier::Ctrl,
            meta: PhysicalModifier::Alt,
            super_: PhysicalModifier::Meta,
        };
        let stroke = KeyStroke::from_physical(
            PhysicalMods::new(true, false, false, false),
            Key::Char('s'),
            &map,
        );
        assert_eq!(stroke, s('s').with_control());
        let mx = KeyStroke::from_physical(
            PhysicalMods::new(false, true, false, false),
            Key::Char('x'),
            &map,
        );
        assert_eq!(mx, s('x').with_meta());
    }

    #[test]
    fn modifier_map_override_swaps_physical_key() {
        let map = ModifierMap::platform_default().with_overrides(Some("meta"), None, None);
        let stroke = KeyStroke::from_physical(
            PhysicalMods::new(false, false, false, true),
            Key::Char('s'),
            &map,
        );
        assert!(stroke.control);
    }

    #[test]
    fn normalizes_aliases() {
        assert_eq!(KeyStroke::parse("esc").unwrap().key, Key::Escape);
        assert_eq!(KeyStroke::parse("ctrl+return").unwrap().key, Key::Enter);
    }

    #[test]
    fn parses_chord_sequence() {
        let chord = parse_chord("ctrl+k ctrl+s").unwrap();
        assert_eq!(chord.len(), 2);
        assert_eq!(chord[0], s('k').with_control());
        assert_eq!(chord[1], s('s').with_control());
    }

    #[test]
    fn resolves_single_binding() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+s", "file.save");
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::Execute("file.save".to_string())
        );
        assert_eq!(
            km.resolve(&[], &s('x').with_control(), None),
            KeymapOutcome::NoMatch
        );
    }

    #[test]
    fn chord_pends_then_executes() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+k ctrl+s", "file.save-all");
        let first = s('k').with_control();
        assert_eq!(km.resolve(&[], &first, None), KeymapOutcome::Pending);
        let pending = vec![first];
        assert_eq!(
            km.resolve(&pending, &s('s').with_control(), None),
            KeymapOutcome::Execute("file.save-all".to_string())
        );
        assert_eq!(
            km.resolve(&pending, &s('x').with_control(), None),
            KeymapOutcome::NoMatch
        );
    }

    #[test]
    fn user_global_overrides_default() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+p", "file.picker");
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+p".to_string(),
            command: "command.palette".to_string(),
            filetype: None,
        }]);
        assert_eq!(
            km.resolve(&[], &s('p').with_control(), None),
            KeymapOutcome::Execute("command.palette".to_string())
        );
    }

    #[test]
    fn display_bindings_reflect_user_overrides() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+p", "file.picker");
        km.bind_default("meta+x", "command.palette");
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+p".to_string(),
            command: "buffer.picker".to_string(),
            filetype: None,
        }]);
        assert_eq!(
            km.display_bindings(None, 2),
            vec![
                ("C-P".to_string(), "buffer.picker".to_string()),
                ("M-X".to_string(), "command.palette".to_string()),
            ]
        );
    }

    #[test]
    fn workspace_search_has_a_default_binding() {
        let km = Keymap::with_defaults();
        let stroke = KeyStroke::parse("ctrl+shift+f").unwrap();
        assert_eq!(
            km.resolve(&[], &stroke, None),
            KeymapOutcome::Execute("search.workspace".to_string())
        );
    }

    #[test]
    fn file_tree_has_a_default_binding() {
        let km = Keymap::with_defaults();
        let stroke = KeyStroke::parse("ctrl+shift+e").unwrap();
        assert_eq!(
            km.resolve(&[], &stroke, None),
            KeymapOutcome::Execute("file.tree".to_string())
        );
    }

    #[test]
    fn continuations_list_next_strokes() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+k ctrl+s", "file.save-all");
        km.bind_default("ctrl+k ctrl+w", "pane.close");
        km.bind_default("ctrl+k r", "file.reload");
        let pending = vec![s('k').with_control()];
        let cont = km.continuations(&pending, None);
        assert_eq!(cont.len(), 3);
        assert!(cont.iter().any(|(k, c)| k == "C-S" && c == "file.save-all"));
        assert!(cont.iter().any(|(k, c)| k == "C-W" && c == "pane.close"));
        assert!(km.continuations(&[s('z').with_control()], None).is_empty());
        assert_eq!(stroke_label(&s('x').with_control()), "C-X");
        assert_eq!(
            stroke_label(&KeyStroke::key(Key::Enter).with_meta()),
            "M-Enter"
        );
    }

    #[test]
    fn continuations_mark_longer_chains_as_prefix() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+k ctrl+x ctrl+s", "deep.command");
        let pending = vec![s('k').with_control()];
        let cont = km.continuations(&pending, None);
        assert_eq!(cont, vec![("C-X".to_string(), "+prefix".to_string())]);
    }

    #[test]
    fn filetype_binding_only_applies_to_matching_filetype() {
        let mut km = Keymap::new();
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+shift+f".to_string(),
            command: "lsp.format".to_string(),
            filetype: Some("rust".to_string()),
        }]);
        let stroke = s('f').with_control().with_shift();
        assert_eq!(
            km.resolve(&[], &stroke, Some("rust")),
            KeymapOutcome::Execute("lsp.format".to_string())
        );
        assert_eq!(
            km.resolve(&[], &stroke, Some("toml")),
            KeymapOutcome::NoMatch
        );
    }
}
