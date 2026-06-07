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
//!
//! `KeyStroke::key` is a normalized lowercase token (`"a"`, `"enter"`,
//! `"right"`, `"f5"`, `"1"`, `"space"`). The GUI maps its platform key codes to
//! these tokens; this crate stays free of any windowing dependency.

use ozone_config::KeymapConfig;

/// A physical modifier key as the platform reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalModifier {
    Ctrl,
    Alt,
    Shift,
    Meta, // the OS "super" key: Windows key / Command
}

/// Physical modifier state for a single key event.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhysicalMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl PhysicalMods {
    pub fn new(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Self {
        Self { ctrl, alt, shift, meta }
    }
    fn has(&self, m: PhysicalModifier) -> bool {
        match m {
            PhysicalModifier::Ctrl => self.ctrl,
            PhysicalModifier::Alt => self.alt,
            PhysicalModifier::Shift => self.shift,
            PhysicalModifier::Meta => self.meta,
        }
    }
}

fn parse_physical_modifier(s: &str) -> Option<PhysicalModifier> {
    match s.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(PhysicalModifier::Ctrl),
        "alt" | "option" => Some(PhysicalModifier::Alt),
        "shift" => Some(PhysicalModifier::Shift),
        "meta" | "super" | "cmd" | "command" | "win" => Some(PhysicalModifier::Meta),
        _ => None,
    }
}

/// Maps Emacs-style logical modifiers to physical keys. Editable per platform.
///
/// Defaults: Control→Ctrl (Cmd on macOS), Meta→Alt, Super→the OS Win/Cmd key.
#[derive(Debug, Clone, Copy)]
pub struct ModifierMap {
    pub control: PhysicalModifier,
    pub meta: PhysicalModifier,
    pub super_: PhysicalModifier,
}

impl ModifierMap {
    pub fn platform_default() -> Self {
        if cfg!(target_os = "macos") {
            // macOS reports Command as the "Meta" key; Control on Cmd is the
            // common editor mapping, Meta(M-) on Option, Super on the Ctrl key.
            Self {
                control: PhysicalModifier::Meta,
                meta: PhysicalModifier::Alt,
                super_: PhysicalModifier::Ctrl,
            }
        } else {
            Self {
                control: PhysicalModifier::Ctrl,
                meta: PhysicalModifier::Alt,
                super_: PhysicalModifier::Meta,
            }
        }
    }

    /// Override individual logical→physical mappings from config tokens.
    pub fn with_overrides(
        mut self,
        control: Option<&str>,
        meta: Option<&str>,
        super_: Option<&str>,
    ) -> Self {
        if let Some(p) = control.and_then(parse_physical_modifier) {
            self.control = p;
        }
        if let Some(p) = meta.and_then(parse_physical_modifier) {
            self.meta = p;
        }
        if let Some(p) = super_.and_then(parse_physical_modifier) {
            self.super_ = p;
        }
        self
    }
}

impl Default for ModifierMap {
    fn default() -> Self {
        Self::platform_default()
    }
}

/// The non-modifier key in a stroke — a structured value, not a raw string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable key, stored lowercase (letters, digits, symbols).
    Char(char),
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    /// Function key F1..=F12.
    F(u8),
}

impl Key {
    /// Parse a key token (`"enter"`, `"f5"`, `"a"`, `"pgdn"`). `None` if unknown.
    pub fn parse(token: &str) -> Option<Self> {
        let t = token.trim().to_ascii_lowercase();
        Some(match t.as_str() {
            "space" | "spc" => Key::Space,
            "enter" | "return" => Key::Enter,
            "escape" | "esc" => Key::Escape,
            "tab" => Key::Tab,
            "backspace" | "bs" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "insert" | "ins" => Key::Insert,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" | "pgup" => Key::PageUp,
            "pagedown" | "pgdn" | "pgdown" => Key::PageDown,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            _ => {
                // F-key, e.g. "f5"
                if let Some(num) = t.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
                    && (1..=12).contains(&num)
                {
                    return Some(Key::F(num));
                }
                // Single printable char.
                let mut chars = t.chars();
                let c = chars.next()?;
                if chars.next().is_none() {
                    Key::Char(c)
                } else {
                    return None;
                }
            }
        })
    }

    /// Human-readable label (for which-key / chord display).
    pub fn label(&self) -> String {
        match self {
            Key::Char(c) => c.to_uppercase().to_string(),
            Key::Space => "Space".into(),
            Key::Enter => "Enter".into(),
            Key::Escape => "Esc".into(),
            Key::Tab => "Tab".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Del".into(),
            Key::Insert => "Ins".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PgUp".into(),
            Key::PageDown => "PgDn".into(),
            Key::Up => "Up".into(),
            Key::Down => "Down".into(),
            Key::Left => "Left".into(),
            Key::Right => "Right".into(),
            Key::F(n) => format!("F{n}"),
        }
    }
}

/// A single chorded key press: Emacs-style *logical* modifiers + a [`Key`].
/// What physical key each modifier means is resolved through a [`ModifierMap`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyStroke {
    pub control: bool,
    pub meta: bool,
    pub super_: bool,
    pub shift: bool,
    pub key: Key,
}

impl KeyStroke {
    /// A modifier-free stroke for `key`.
    pub fn key(key: Key) -> Self {
        Self { control: false, meta: false, super_: false, shift: false, key }
    }

    pub fn with_control(mut self) -> Self { self.control = true; self }
    pub fn with_meta(mut self) -> Self { self.meta = true; self }
    pub fn with_super(mut self) -> Self { self.super_ = true; self }
    pub fn with_shift(mut self) -> Self { self.shift = true; self }

    /// Build a logical stroke from a physical key event via the modifier map.
    pub fn from_physical(phys: PhysicalMods, key: Key, map: &ModifierMap) -> Self {
        Self {
            control: phys.has(map.control),
            meta: phys.has(map.meta),
            super_: phys.has(map.super_),
            shift: phys.shift,
            key,
        }
    }

    /// Parse one stroke token like `"ctrl+shift+f"` or `"meta+x"`. Modifier
    /// tokens: `ctrl`/`control`, `meta`/`alt`/`option`, `super`/`cmd`/`win`,
    /// `shift`. Returns `None` if there is no recognized key part.
    pub fn parse(token: &str) -> Option<Self> {
        let mut control = false;
        let mut meta = false;
        let mut super_ = false;
        let mut shift = false;
        let mut key = None;
        for part in token.split('+') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => control = true,
                "meta" | "alt" | "option" => meta = true,
                "super" | "cmd" | "command" | "win" => super_ = true,
                "shift" => shift = true,
                other => key = Key::parse(other),
            }
        }
        Some(Self { control, meta, super_, shift, key: key? })
    }
}

/// Human-readable label for a single stroke (`"C-x"`, `"M-g"`, `"Enter"`),
/// Emacs-style modifier prefixes. For which-key / chord display.
pub fn stroke_label(stroke: &KeyStroke) -> String {
    let mut s = String::new();
    if stroke.control {
        s.push_str("C-");
    }
    if stroke.meta {
        s.push_str("M-");
    }
    if stroke.super_ {
        s.push_str("s-");
    }
    if stroke.shift {
        s.push_str("S-");
    }
    s.push_str(&stroke.key.label());
    s
}

/// Parse a full chord string like `"ctrl+k ctrl+s"` into its strokes.
pub fn parse_chord(keys: &str) -> Option<Vec<KeyStroke>> {
    let strokes: Vec<KeyStroke> = keys.split_whitespace().filter_map(KeyStroke::parse).collect();
    if strokes.is_empty() { None } else { Some(strokes) }
}

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
        Self { bindings: Vec::new() }
    }

    /// The shipped default layer. These are the keys Ozone binds out of the box;
    /// user `[[keymap]]` entries layer on top and can override any of them.
    pub fn with_defaults() -> Self {
        let mut km = Self::new();
        let defaults: &[(&str, &str)] = &[
            // File / edit / buffer
            ("ctrl+s", "file.save"),
            ("ctrl+z", "edit.undo"),
            ("ctrl+y", "edit.redo"),
            ("ctrl+p", "file.picker"),
            ("alt+x", "command.palette"),       // Emacs M-x
            ("ctrl+shift+p", "command.palette"),
            ("ctrl+tab", "buffer.next"),
            ("ctrl+shift+tab", "buffer.previous"),
            ("ctrl+x b", "buffer.picker"),      // Emacs switch-buffer
            ("meta+left", "view.jump-back"),    // jump list back  (VS Code Alt+Left)
            ("meta+right", "view.jump-forward"),// jump list forward (Alt+Right)
            ("ctrl+-", "view.jump-back"),       // plan binding (now that '-' is a keycode)
            ("ctrl+=", "view.jump-forward"),
            ("ctrl+k ctrl+s", "file.save-all"), // chord showcase
            // Panes
            ("ctrl+shift+right", "pane.split-right"),
            ("ctrl+shift+down", "pane.split-down"),
            ("ctrl+shift+w", "pane.close"),
            ("ctrl+alt+right", "pane.focus-right"),
            ("ctrl+alt+left", "pane.focus-left"),
            ("ctrl+alt+down", "pane.focus-down"),
            ("ctrl+alt+up", "pane.focus-up"),
            // Emacs-style movement
            ("ctrl+a", "cursor.line-start"),
            ("ctrl+e", "cursor.line-end"),
            ("ctrl+b", "cursor.move-left"),
            ("ctrl+f", "cursor.move-right"),
            ("ctrl+n", "cursor.move-down"),
            ("meta+f", "search.start"), // M-f opens in-buffer find
            ("meta+h", "search.replace"), // M-h opens find with a replace box
            ("ctrl+shift+f", "search.workspace"),
            ("meta+g", "edit.goto-line"),       // Emacs M-g — prompt for a line
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
            let layer = if cfg.filetype.is_some() { Layer::Filetype } else { Layer::Global };
            self.bindings.push(Binding {
                chord,
                command: cfg.command.clone(),
                filetype: cfg.filetype.clone(),
                layer,
            });
        }
    }

    /// Whether this binding applies given the active filetype.
    fn applies(binding: &Binding, filetype: Option<&str>) -> bool {
        match &binding.filetype {
            None => true,
            Some(ft) => filetype == Some(ft.as_str()),
        }
    }

    /// Possible continuations of a pending chord prefix, for a which-key popup.
    ///
    /// Given the strokes already typed, returns one entry per *distinct next
    /// stroke* that could extend `pending` into a binding: the next stroke's
    /// label and either the command it runs (when that stroke completes a
    /// binding) or `+prefix` (when it only leads to longer bindings). Highest
    /// layer wins on ties; results are sorted by label for a stable display.
    pub fn continuations(
        &self,
        pending: &[KeyStroke],
        filetype: Option<&str>,
    ) -> Vec<(String, String)> {
        use std::collections::BTreeMap;
        // next-stroke label -> (best layer seen, description)
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
            // A binding that ends right here names a command; a longer one is a
            // further prefix (group).
            let desc = if b.chord.len() == pending.len() + 1 {
                b.command.clone()
            } else {
                "+prefix".to_string()
            };
            next.entry(label)
                .and_modify(|(layer, d)| {
                    // Prefer a concrete command over a group, then higher layer.
                    if b.layer > *layer || (*d == "+prefix" && desc != "+prefix") {
                        *layer = b.layer;
                        *d = desc.clone();
                    }
                })
                .or_insert((b.layer, desc));
        }
        next.into_iter().map(|(k, (_, d))| (k, d)).collect()
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

        // A longer binding could still match → wait for more input.
        let has_longer = self.bindings.iter().any(|b| {
            Self::applies(b, filetype) && b.chord.len() > seq.len() && b.chord[..seq.len()] == seq[..]
        });
        if has_longer {
            return KeymapOutcome::Pending;
        }

        // Exact match — pick the highest-priority layer.
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
        assert!(KeyStroke::parse("ctrl").is_none()); // no key part
        assert_eq!(KeyStroke::parse("ctrl+k ctrl+s").is_none(), false);
        assert_eq!(Key::parse("f5"), Some(Key::F(5)));
        assert_eq!(Key::parse("pgdn"), Some(Key::PageDown));
        // alt and meta are the same logical modifier (Emacs M-)
        assert_eq!(KeyStroke::parse("alt+x"), KeyStroke::parse("meta+x"));
        assert!(KeyStroke::parse("super+p").unwrap().super_);
    }

    #[test]
    fn modifier_map_resolves_logical_from_physical() {
        let map = ModifierMap {
            control: PhysicalModifier::Ctrl,
            meta: PhysicalModifier::Alt,
            super_: PhysicalModifier::Meta,
        };
        // physical Ctrl+s -> logical control
        let stroke = KeyStroke::from_physical(PhysicalMods::new(true, false, false, false), Key::Char('s'), &map);
        assert_eq!(stroke, s('s').with_control());
        // physical Alt+x -> logical meta (M-x)
        let mx = KeyStroke::from_physical(PhysicalMods::new(false, true, false, false), Key::Char('x'), &map);
        assert_eq!(mx, s('x').with_meta());
    }

    #[test]
    fn modifier_map_override_swaps_physical_key() {
        // Make logical Control map to the OS Meta/Cmd key.
        let map = ModifierMap::platform_default().with_overrides(Some("meta"), None, None);
        // physical Meta(cmd)+s now yields logical control
        let stroke = KeyStroke::from_physical(PhysicalMods::new(false, false, false, true), Key::Char('s'), &map);
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
        assert_eq!(km.resolve(&[], &s('x').with_control(), None), KeymapOutcome::NoMatch);
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
        // Wrong continuation cancels the chord.
        assert_eq!(km.resolve(&pending, &s('x').with_control(), None), KeymapOutcome::NoMatch);
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
    fn workspace_search_has_a_default_binding() {
        let km = Keymap::with_defaults();
        let stroke = KeyStroke::parse("ctrl+shift+f").unwrap();
        assert_eq!(
            km.resolve(&[], &stroke, None),
            KeymapOutcome::Execute("search.workspace".to_string())
        );
    }

    #[test]
    fn continuations_list_next_strokes() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+k ctrl+s", "file.save-all");
        km.bind_default("ctrl+k ctrl+w", "pane.close");
        km.bind_default("ctrl+k r", "file.reload"); // 2-stroke group? no, completes
        // Pending C-k: three next strokes, each completing a binding.
        let pending = vec![s('k').with_control()];
        let cont = km.continuations(&pending, None);
        assert_eq!(cont.len(), 3);
        // Key::label uppercases printable chars (used for chord display).
        assert!(cont.iter().any(|(k, c)| k == "C-S" && c == "file.save-all"));
        assert!(cont.iter().any(|(k, c)| k == "C-W" && c == "pane.close"));
        // No pending prefix and an empty match → empty.
        assert!(km.continuations(&[s('z').with_control()], None).is_empty());
        assert_eq!(stroke_label(&s('x').with_control()), "C-X");
        assert_eq!(stroke_label(&KeyStroke::key(Key::Enter).with_meta()), "M-Enter");
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
        assert_eq!(km.resolve(&[], &stroke, Some("toml")), KeymapOutcome::NoMatch);
    }
}
