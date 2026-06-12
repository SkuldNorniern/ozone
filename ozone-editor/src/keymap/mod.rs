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

use std::collections::BTreeMap;

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

/// The shipped default bindings (lowest layer). Kept in sync with the generated
/// `keymap.toml` template (the `shipped_keymap_matches_defaults` test guards it).
///
/// Super (Win key / macOS Option) is intentionally unbound — it is OS-reserved,
/// matching Emacs/Neovim leaving the GUI super key to the platform. macOS
/// Command is Meta, so the Meta bindings are reachable as Cmd-… there.
pub const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    // File / edit
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
    ("ctrl+shift+o", "symbol.picker"),
    ("ctrl+k ctrl+l", "fold.toggle"),
    ("ctrl+k ctrl+0", "fold.open-all"),
    ("ctrl+k ctrl+j", "fold.all"),
    ("ctrl+k ctrl+s", "file.save-all"),
    // Navigation history
    ("meta+left", "view.jump-back"),
    ("meta+right", "view.jump-forward"),
    ("ctrl+-", "view.jump-back"),
    ("ctrl+=", "view.jump-forward"),
    // Panes
    ("ctrl+shift+right", "pane.split-right"),
    ("ctrl+shift+down", "pane.split-down"),
    ("ctrl+shift+w", "pane.close"),
    ("ctrl+meta+right", "pane.focus-right"),
    ("ctrl+meta+left", "pane.focus-left"),
    ("ctrl+meta+down", "pane.focus-down"),
    ("ctrl+meta+up", "pane.focus-up"),
    // Emacs-style movement (Ctrl / Meta)
    ("ctrl+a", "cursor.line-start"),
    ("ctrl+e", "cursor.line-end"),
    ("ctrl+b", "cursor.move-left"),
    ("ctrl+f", "cursor.move-right"),
    ("ctrl+n", "cursor.move-down"),
    ("meta+f", "search.start"),
    ("meta+h", "search.replace"),
    ("ctrl+shift+f", "search.workspace"),
    ("meta+g", "edit.goto-line"),
    ("meta+.", "lsp.goto-definition"),
    ("meta+k", "lsp.hover"),
    ("ctrl+space", "lsp.completion"),
    ("ctrl+home", "cursor.file-start"),
    ("ctrl+end", "cursor.file-end"),
    ("ctrl+left", "cursor.word-backward"),
    ("ctrl+right", "cursor.word-forward"),
    // Plain navigation
    ("up", "cursor.move-up"),
    ("down", "cursor.move-down"),
    ("left", "cursor.move-left"),
    ("right", "cursor.move-right"),
    ("home", "cursor.line-start"),
    ("end", "cursor.line-end"),
    ("pageup", "view.page-up"),
    ("pagedown", "view.page-down"),
    // Editing
    ("backspace", "edit.delete-char-backward"),
    ("delete", "edit.delete-char-forward"),
    ("enter", "edit.insert-newline"),
    ("ctrl+/", "edit.toggle-comment"),
    ("ctrl+d", "edit.duplicate-line"),
    ("meta+up", "edit.move-line-up"),
    ("meta+down", "edit.move-line-down"),
    // Clipboard — ctrl+c/x/v (Windows/Linux); meta+c/x/v = Cmd (macOS)
    ("ctrl+c", "edit.copy"),
    ("ctrl+x", "edit.cut"),
    ("ctrl+v", "edit.paste"),
    ("meta+c", "edit.copy"),
    ("meta+x", "edit.cut"),
    ("meta+v", "edit.paste"),
    // Selection
    ("meta+shift+up", "select.expand"),
];

impl Keymap {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// A keymap seeded with [`DEFAULT_BINDINGS`]. **Not used at runtime** — the
    /// app builds its keymap purely from config (`keymap.toml`, generated on
    /// first launch from these same defaults), so removing a binding there
    /// unbinds it. This is a convenience/seed for tests and embedders.
    pub fn with_defaults() -> Self {
        let mut km = Self::new();
        for (keys, cmd) in DEFAULT_BINDINGS {
            km.bind_default(keys, cmd);
        }
        km
    }

    /// Add a shipped default binding (lowest priority).
    pub fn bind_default(&mut self, keys: &str, command: &str) {
        let chord = parse_chord(keys)
            .unwrap_or_else(|| panic!("invalid shipped default binding: {keys:?}"));
        self.bindings.push(Binding {
            chord,
            command: command.to_string(),
            filetype: None,
            layer: Layer::Default,
        });
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

    /// Insert or upgrade a which-key entry: higher layer wins; a resolved
    /// command beats a `"+prefix"` placeholder at the same layer.
    fn upgrade_entry(
        map: &mut BTreeMap<String, (Layer, String)>,
        label: String,
        layer: Layer,
        desc: String,
    ) {
        map.entry(label)
            .and_modify(|(prev_layer, prev_desc)| {
                if layer > *prev_layer || (*prev_desc == "+prefix" && desc != "+prefix") {
                    *prev_layer = layer;
                    *prev_desc = desc.clone();
                }
            })
            .or_insert((layer, desc));
    }

    /// Possible continuations of a pending chord prefix, for a which-key popup.
    pub fn continuations(
        &self,
        pending: &[KeyStroke],
        filetype: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut next = BTreeMap::new();
        for b in &self.bindings {
            if !Self::applies(b, filetype) || b.chord.len() <= pending.len() {
                continue;
            }
            if b.chord[..pending.len()] != *pending {
                continue;
            }
            let stroke = &b.chord[pending.len()];
            let desc = if b.chord.len() == pending.len() + 1 {
                b.command.clone()
            } else {
                "+prefix".to_string()
            };
            Self::upgrade_entry(&mut next, stroke_label(stroke), b.layer, desc);
        }
        next.into_iter().map(|(k, (_, d))| (k, d)).collect()
    }

    /// First strokes reachable while only the given modifiers are held, for the
    /// bare-modifier which-key hint (e.g. holding `Ctrl` lists every `C-…`
    /// binding). A binding's leading stroke must match the held `control` /
    /// `meta` / `super_` flags exactly; `shift` is left free so `C-S-e` still
    /// appears while only `Ctrl` is held. Returns `(stroke_label, command-or-+prefix)`.
    pub fn modifier_continuations(
        &self,
        control: bool,
        meta: bool,
        super_: bool,
        filetype: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut next = BTreeMap::new();
        for b in &self.bindings {
            if !Self::applies(b, filetype) {
                continue;
            }
            let Some(stroke) = b.chord.first() else {
                continue;
            };
            if stroke.control != control || stroke.meta != meta || stroke.super_ != super_ {
                continue;
            }
            let desc = if b.chord.len() == 1 {
                b.command.clone()
            } else {
                "+prefix".to_string()
            };
            Self::upgrade_entry(&mut next, stroke_label(stroke), b.layer, desc);
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
    ///
    /// Single-pass: no allocation. A longer matching binding always beats an
    /// exact match (Emacs-style — wait for the full chord before executing).
    pub fn resolve(
        &self,
        pending: &[KeyStroke],
        stroke: &KeyStroke,
        filetype: Option<&str>,
    ) -> KeymapOutcome {
        let seq_len = pending.len() + 1;
        let mut best: Option<&Binding> = None;
        let mut has_longer = false;

        for b in &self.bindings {
            if !Self::applies(b, filetype) {
                continue;
            }
            let chord = &b.chord;
            if chord.len() < seq_len {
                continue;
            }
            if chord[..pending.len()] != *pending || chord[pending.len()] != *stroke {
                continue;
            }
            if chord.len() == seq_len {
                if best.is_none_or(|prev| b.layer > prev.layer) {
                    best = Some(b);
                }
            } else {
                has_longer = true;
            }
        }

        if has_longer {
            KeymapOutcome::Pending
        } else if let Some(b) = best {
            KeymapOutcome::Execute(b.command.clone())
        } else {
            KeymapOutcome::NoMatch
        }
    }
}

#[cfg(test)]
mod tests {
    use ozone_config::Config;

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
        assert!(KeyStroke::parse("ctrl+k ctrl+s").is_some());
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
    fn keymap_is_purely_config_driven() {
        // A fresh keymap has nothing bound — there is no hidden default layer.
        let km = Keymap::new();
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::NoMatch
        );
        // Config (as keymap.toml would supply) binds it…
        let mut km = Keymap::new();
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+s".to_string(),
            command: "file.save".to_string(),
            filetype: None,
        }]);
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::Execute("file.save".to_string())
        );
        // …and omitting it from config leaves it unbound, even though `ctrl+s`
        // is a shipped default — defaults are not hardcoded at runtime.
        let mut km = Keymap::new();
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+z".to_string(),
            command: "edit.undo".to_string(),
            filetype: None,
        }]);
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::NoMatch
        );
    }

    #[test]
    fn shipped_keymap_matches_defaults() {
        // The generated `keymap.toml` template must list exactly the built-in
        // defaults — guards the hand-maintained file against drift.
        use std::collections::BTreeSet;
        let text = include_str!("../../../keymap.toml");
        let cfg = Config::parse_str(text);
        let from_file: BTreeSet<(String, String)> = cfg
            .keymaps
            .iter()
            .map(|k| (k.keys.clone(), k.command.clone()))
            .collect();
        let from_const: BTreeSet<(String, String)> = DEFAULT_BINDINGS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            from_file, from_const,
            "keymap.toml has drifted from DEFAULT_BINDINGS"
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
    fn modifier_continuations_list_bare_prefix_bindings() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+s", "file.save");
        km.bind_default("ctrl+shift+e", "file.tree");
        km.bind_default("ctrl+k ctrl+s", "file.save-all");
        km.bind_default("meta+x", "command.palette");
        km.bind_default("ctrl+meta+right", "pane.focus-right");

        let ctrl = km.modifier_continuations(true, false, false, None);
        // C-S leaf, C-S-E leaf (shift free), C-K as a +prefix group.
        assert!(ctrl.iter().any(|(k, c)| k == "C-S" && c == "file.save"));
        assert!(ctrl.iter().any(|(k, c)| k == "C-S-E" && c == "file.tree"));
        assert!(ctrl.iter().any(|(k, c)| k == "C-K" && c == "+prefix"));
        // Combos needing another modifier (meta) are excluded while only Ctrl is held.
        assert!(ctrl.iter().all(|(k, _)| k != "C-M-Right"));

        let meta = km.modifier_continuations(false, true, false, None);
        assert_eq!(
            meta,
            vec![("M-X".to_string(), "command.palette".to_string())]
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
