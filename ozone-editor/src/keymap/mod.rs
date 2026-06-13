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

/// A user keybinding that overwrites an earlier binding in the same scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapConflict {
    pub chord: String,
    pub filetype: Option<String>,
    pub previous_command: String,
    pub command: String,
}

impl KeymapConflict {
    pub fn message(&self) -> String {
        let scope = self.filetype.as_deref().unwrap_or("global");
        format!(
            "{} ({scope}): {} is overwritten by {}",
            self.chord, self.previous_command, self.command
        )
    }
}

/// A user binding that was dropped while layering config, with the reason.
/// Surfaced at startup so a typo in `keymap.toml` is visible instead of the
/// binding vanishing silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapWarning {
    /// The chord text could not be parsed into key strokes (unknown key name,
    /// bare modifier, malformed combo).
    InvalidChord { keys: String, command: String },
    /// The `platform` value is not one of `macos` / `windows` / `linux`, so the
    /// binding was dropped on every platform.
    UnknownPlatform {
        platform: String,
        keys: String,
        command: String,
    },
}

impl KeymapWarning {
    pub fn message(&self) -> String {
        match self {
            KeymapWarning::InvalidChord { keys, command } => {
                format!("{keys:?} -> {command}: not a valid key chord — binding ignored")
            }
            KeymapWarning::UnknownPlatform {
                platform,
                keys,
                command,
            } => format!(
                "{keys} -> {command}: unknown platform {platform:?} \
                 (expected macos / windows / linux) — binding ignored"
            ),
        }
    }
}

/// Problems found while layering user keymap config onto the defaults.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeymapReport {
    /// Bindings that overwrite an earlier binding in the same scope.
    pub conflicts: Vec<KeymapConflict>,
    /// Bindings dropped because they were malformed (bad chord / platform).
    pub warnings: Vec<KeymapWarning>,
}

impl KeymapReport {
    /// No problems found.
    pub fn is_empty(&self) -> bool {
        self.conflicts.is_empty() && self.warnings.is_empty()
    }
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
    // Clipboard — ctrl+c/x/v universal; meta+c/x/v are macOS-only (see MACOS_BINDINGS)
    ("ctrl+c", "edit.copy"),
    ("ctrl+x", "edit.cut"),
    ("ctrl+v", "edit.paste"),
    // Selection — text-object
    ("meta+shift+up", "select.expand"),
    // Keyboard extend-selection (shift+arrow family)
    ("shift+left", "select.extend-left"),
    ("shift+right", "select.extend-right"),
    ("shift+up", "select.extend-up"),
    ("shift+down", "select.extend-down"),
    ("shift+home", "select.extend-line-start"),
    ("shift+end", "select.extend-line-end"),
    ("ctrl+shift+left", "select.extend-word-backward"),
    ("ctrl+shift+right", "select.extend-word-forward"),
    ("ctrl+shift+home", "select.extend-file-start"),
    ("ctrl+shift+end", "select.extend-file-end"),
    ("shift+pageup", "select.extend-page-up"),
    ("shift+pagedown", "select.extend-page-down"),
];

/// macOS-specific default bindings (applied only on `target_os = "macos"`).
/// `meta` = Cmd on macOS, so these are the standard Cmd+C/X/V clipboard
/// shortcuts and a `meta+space` completion alternative that avoids the macOS
/// input-source-switcher collision on `ctrl+space`.
pub const MACOS_BINDINGS: &[(&str, &str)] = &[
    ("meta+c", "edit.copy"),
    ("meta+x", "edit.cut"),
    ("meta+v", "edit.paste"),
    ("meta+space", "lsp.completion"),
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
    ///
    /// Entries whose `platform` names a recognized but non-matching OS are
    /// silently skipped, so the same `keymap.toml` works across platforms
    /// without editing. Entries with an *unrecognized* `platform` value or an
    /// unparseable chord are dropped and reported in the returned
    /// [`KeymapReport`] rather than vanishing silently.
    pub fn add_user_config(&mut self, configs: &[KeymapConfig]) -> KeymapReport {
        let mut report = KeymapReport::default();
        for cfg in configs {
            if let Some(platform) = &cfg.platform {
                match platform.trim().to_ascii_lowercase().as_str() {
                    "macos" => {
                        if !cfg!(target_os = "macos") {
                            continue;
                        }
                    }
                    "windows" => {
                        if !cfg!(target_os = "windows") {
                            continue;
                        }
                    }
                    "linux" => {
                        if !cfg!(target_os = "linux") {
                            continue;
                        }
                    }
                    _ => {
                        report.warnings.push(KeymapWarning::UnknownPlatform {
                            platform: platform.clone(),
                            keys: cfg.keys.clone(),
                            command: cfg.command.clone(),
                        });
                        continue;
                    }
                }
            }
            let Some(chord) = parse_chord(&cfg.keys) else {
                report.warnings.push(KeymapWarning::InvalidChord {
                    keys: cfg.keys.clone(),
                    command: cfg.command.clone(),
                });
                continue;
            };
            let layer = if cfg.filetype.is_some() {
                Layer::Filetype
            } else {
                Layer::Global
            };
            if let Some(previous) = self.bindings.iter().rev().find(|binding| {
                binding.layer == layer && binding.filetype == cfg.filetype && binding.chord == chord
            }) {
                report.conflicts.push(KeymapConflict {
                    chord: chord_label(&chord),
                    filetype: cfg.filetype.clone(),
                    previous_command: previous.command.clone(),
                    command: cfg.command.clone(),
                });
            }
            self.bindings.push(Binding {
                chord,
                command: cfg.command.clone(),
                filetype: cfg.filetype.clone(),
                layer,
            });
        }
        report
    }

    /// Distinct bound command ids for which `is_known` returns `false`, sorted
    /// and de-duplicated. Used to warn about bindings that point at a command
    /// the registry doesn't have. `is_known` is supplied by the frontend so the
    /// keymap stays agnostic of the command registry and of shell sigils
    /// (`|cmd` / `!cmd`) — both are "known" binding targets.
    pub fn unknown_commands(&self, is_known: impl Fn(&str) -> bool) -> Vec<String> {
        let mut unknown: Vec<&str> = self
            .bindings
            .iter()
            .map(|b| b.command.as_str())
            .filter(|c| !is_known(c))
            .collect();
        unknown.sort_unstable();
        unknown.dedup();
        unknown.into_iter().map(str::to_string).collect()
    }

    /// Complete bindings that can never fire because a longer binding in an
    /// overlapping scope makes their chord a prefix — resolution always waits
    /// for the longer chord ([`KeymapOutcome::Pending`]), so the shorter
    /// command is dead. Returns each shadowed chord's label once, sorted.
    ///
    /// Two scopes overlap when either is global or they name the same filetype:
    /// a binding scoped to one filetype does not shadow one in another, since
    /// they never resolve together.
    pub fn shadowed_by_longer_chord(&self) -> Vec<String> {
        fn scopes_overlap(a: &Option<String>, b: &Option<String>) -> bool {
            a.is_none() || b.is_none() || a == b
        }

        let mut shadowed: Vec<String> = Vec::new();
        for short in &self.bindings {
            let n = short.chord.len();
            let is_prefix_of_longer = self.bindings.iter().any(|long| {
                long.chord.len() > n
                    && long.chord[..n] == short.chord[..]
                    && scopes_overlap(&short.filetype, &long.filetype)
            });
            if is_prefix_of_longer {
                shadowed.push(chord_label(&short.chord));
            }
        }
        shadowed.sort_unstable();
        shadowed.dedup();
        shadowed
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
                // Later entries at the same layer override earlier ones. This
                // lets platform-specific config refine a portable binding.
                if best.is_none_or(|prev| b.layer >= prev.layer) {
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
        assert!(KeyStroke::parse("ctrl+k ctrl+s").is_none());
        assert!(parse_chord("ctrl+k ctrl+s").is_some());
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
    fn rejects_chords_with_invalid_strokes() {
        assert!(parse_chord("ctrl+k definitely-not-a-key ctrl+s").is_none());
        assert!(parse_chord("ctrl+bogus+k").is_none());
        assert!(parse_chord("ctrl+k+s").is_none());
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
            platform: None,
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
            platform: None,
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
            platform: None,
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
            platform: None,
        }]);
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::NoMatch
        );
    }

    #[test]
    fn shipped_keymap_matches_defaults() {
        // keymap.toml must mirror DEFAULT_BINDINGS (platform-agnostic entries)
        // and MACOS_BINDINGS (entries with `platform = "macos"`). Guards both
        // constants against drift from the hand-maintained file.
        use std::collections::BTreeSet;
        let text = include_str!("../../../keymap.toml");
        let cfg = Config::parse_str(text);

        let from_file_global: BTreeSet<(String, String)> = cfg
            .keymaps
            .iter()
            .filter(|k| k.platform.is_none())
            .map(|k| (k.keys.clone(), k.command.clone()))
            .collect();
        let from_const_global: BTreeSet<(String, String)> = DEFAULT_BINDINGS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            from_file_global, from_const_global,
            "keymap.toml global entries have drifted from DEFAULT_BINDINGS"
        );

        let from_file_macos: BTreeSet<(String, String)> = cfg
            .keymaps
            .iter()
            .filter(|k| k.platform.as_deref() == Some("macos"))
            .map(|k| (k.keys.clone(), k.command.clone()))
            .collect();
        let from_const_macos: BTreeSet<(String, String)> = MACOS_BINDINGS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            from_file_macos, from_const_macos,
            "keymap.toml macos entries have drifted from MACOS_BINDINGS"
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
            platform: None,
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

    #[test]
    fn later_binding_wins_within_the_same_layer() {
        let mut km = Keymap::new();
        let report = km.add_user_config(&[
            KeymapConfig {
                keys: "meta+x".to_string(),
                command: "command.palette".to_string(),
                filetype: None,
                platform: None,
            },
            KeymapConfig {
                keys: "meta+x".to_string(),
                command: "edit.cut".to_string(),
                filetype: None,
                platform: Some(std::env::consts::OS.to_string()),
            },
        ]);

        let stroke = KeyStroke::parse("meta+x").unwrap();
        assert_eq!(
            km.resolve(&[], &stroke, None),
            KeymapOutcome::Execute("edit.cut".to_string())
        );
        assert_eq!(
            report.conflicts,
            vec![KeymapConflict {
                chord: "M-X".to_string(),
                filetype: None,
                previous_command: "command.palette".to_string(),
                command: "edit.cut".to_string(),
            }]
        );
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn global_and_filetype_bindings_are_not_duplicates() {
        let mut km = Keymap::new();
        let report = km.add_user_config(&[
            KeymapConfig {
                keys: "ctrl+s".to_string(),
                command: "file.save".to_string(),
                filetype: None,
                platform: None,
            },
            KeymapConfig {
                keys: "control+s".to_string(),
                command: "rust.save".to_string(),
                filetype: Some("rust".to_string()),
                platform: None,
            },
        ]);

        assert!(report.is_empty());
    }

    #[test]
    fn invalid_chord_is_reported_not_silently_dropped() {
        let mut km = Keymap::new();
        let report = km.add_user_config(&[
            KeymapConfig {
                keys: "ctrl+definitely-not-a-key".to_string(),
                command: "edit.undo".to_string(),
                filetype: None,
                platform: None,
            },
            KeymapConfig {
                keys: "ctrl+s".to_string(),
                command: "file.save".to_string(),
                filetype: None,
                platform: None,
            },
        ]);

        // The valid binding still applies; the malformed one is reported.
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::Execute("file.save".to_string())
        );
        assert_eq!(
            report.warnings,
            vec![KeymapWarning::InvalidChord {
                keys: "ctrl+definitely-not-a-key".to_string(),
                command: "edit.undo".to_string(),
            }]
        );
    }

    #[test]
    fn unknown_platform_is_reported_not_silently_dropped() {
        let mut km = Keymap::new();
        let report = km.add_user_config(&[KeymapConfig {
            keys: "ctrl+s".to_string(),
            command: "file.save".to_string(),
            filetype: None,
            platform: Some("beos".to_string()),
        }]);

        // Dropped on every platform (we can't know which OS it meant)…
        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::NoMatch
        );
        // …but reported instead of vanishing.
        assert_eq!(
            report.warnings,
            vec![KeymapWarning::UnknownPlatform {
                platform: "beos".to_string(),
                keys: "ctrl+s".to_string(),
                command: "file.save".to_string(),
            }]
        );
    }

    #[test]
    fn unknown_commands_lists_bindings_to_missing_commands() {
        let mut km = Keymap::new();
        km.add_user_config(&[
            KeymapConfig {
                keys: "ctrl+s".to_string(),
                command: "file.save".to_string(),
                filetype: None,
                platform: None,
            },
            KeymapConfig {
                keys: "ctrl+q".to_string(),
                command: "totally.bogus".to_string(),
                filetype: None,
                platform: None,
            },
            KeymapConfig {
                keys: "ctrl+r".to_string(),
                command: "|rustfmt".to_string(),
                filetype: None,
                platform: None,
            },
        ]);
        // `file.save` is "known", `|rustfmt` is a sigil the closure accepts;
        // only the bogus id is reported.
        let known = |name: &str| name == "file.save" || name.starts_with('|');
        assert_eq!(
            km.unknown_commands(known),
            vec!["totally.bogus".to_string()]
        );
    }

    #[test]
    fn shadowed_by_longer_chord_finds_prefix_commands() {
        let mut km = Keymap::new();
        // `ctrl+k` is both a command and the prefix of `ctrl+k ctrl+s`: the
        // bare `ctrl+k` can never fire.
        km.bind_default("ctrl+k", "some.command");
        km.bind_default("ctrl+k ctrl+s", "file.save-all");
        // `ctrl+s` is a leaf with no longer sibling — fine.
        km.bind_default("ctrl+s", "file.save");
        assert_eq!(km.shadowed_by_longer_chord(), vec!["C-K".to_string()]);
    }

    #[test]
    fn shadowed_check_respects_filetype_scope() {
        let mut km = Keymap::new();
        // A rust-only leaf and a toml-only longer chord never resolve together,
        // so the leaf is not shadowed.
        km.add_user_config(&[
            KeymapConfig {
                keys: "ctrl+k".to_string(),
                command: "rust.thing".to_string(),
                filetype: Some("rust".to_string()),
                platform: None,
            },
            KeymapConfig {
                keys: "ctrl+k ctrl+s".to_string(),
                command: "toml.thing".to_string(),
                filetype: Some("toml".to_string()),
                platform: None,
            },
        ]);
        assert!(km.shadowed_by_longer_chord().is_empty());

        // But a global longer chord shadows the rust leaf (global applies in
        // rust buffers too).
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+k ctrl+w".to_string(),
            command: "global.thing".to_string(),
            filetype: None,
            platform: None,
        }]);
        assert_eq!(km.shadowed_by_longer_chord(), vec!["C-K".to_string()]);
    }

    #[test]
    fn recognized_nonmatching_platform_is_skipped_without_a_warning() {
        // Pick a recognized OS that is *not* the current one.
        let other = if cfg!(target_os = "windows") {
            "linux"
        } else {
            "windows"
        };
        let mut km = Keymap::new();
        let report = km.add_user_config(&[KeymapConfig {
            keys: "ctrl+s".to_string(),
            command: "file.save".to_string(),
            filetype: None,
            platform: Some(other.to_string()),
        }]);

        assert_eq!(
            km.resolve(&[], &s('s').with_control(), None),
            KeymapOutcome::NoMatch
        );
        // A deliberate cross-platform entry is not a problem — no warning.
        assert!(report.is_empty());
    }
}
