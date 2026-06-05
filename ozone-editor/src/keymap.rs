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

/// A single chorded key press: modifiers + a normalized key token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyStroke {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: String,
}

impl KeyStroke {
    /// A modifier-free stroke for `key` (already normalized/lowercase).
    pub fn key(key: impl Into<String>) -> Self {
        Self { ctrl: false, alt: false, shift: false, meta: false, key: key.into() }
    }

    pub fn with_ctrl(mut self) -> Self { self.ctrl = true; self }
    pub fn with_alt(mut self) -> Self { self.alt = true; self }
    pub fn with_shift(mut self) -> Self { self.shift = true; self }
    pub fn with_meta(mut self) -> Self { self.meta = true; self }

    /// Parse one stroke token like `"ctrl+shift+f"`. Returns `None` if there is
    /// no key part (e.g. just `"ctrl"`).
    pub fn parse(token: &str) -> Option<Self> {
        let mut stroke = KeyStroke {
            ctrl: false, alt: false, shift: false, meta: false, key: String::new(),
        };
        let mut have_key = false;
        for part in token.split('+') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => stroke.ctrl = true,
                "alt" | "option" => stroke.alt = true,
                "shift" => stroke.shift = true,
                "meta" | "super" | "cmd" | "command" | "win" => stroke.meta = true,
                other => {
                    stroke.key = normalize_key(other);
                    have_key = true;
                }
            }
        }
        if have_key && !stroke.key.is_empty() {
            Some(stroke)
        } else {
            None
        }
    }
}

/// Normalize a key token to the canonical form used at runtime.
fn normalize_key(key: &str) -> String {
    match key {
        "esc" => "escape".to_string(),
        "return" => "enter".to_string(),
        "pgup" => "pageup".to_string(),
        "pgdn" | "pgdown" => "pagedown".to_string(),
        "del" => "delete".to_string(),
        "bs" => "backspace".to_string(),
        "spc" => "space".to_string(),
        other => other.to_string(),
    }
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
            ("ctrl+tab", "buffer.next"),
            ("ctrl+shift+tab", "buffer.previous"),
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

    fn s(key: &str) -> KeyStroke {
        KeyStroke::key(key)
    }

    #[test]
    fn parses_modifiers_and_key() {
        let k = KeyStroke::parse("ctrl+shift+f").unwrap();
        assert!(k.ctrl && k.shift && !k.alt);
        assert_eq!(k.key, "f");
        assert!(KeyStroke::parse("ctrl").is_none()); // no key part
    }

    #[test]
    fn normalizes_aliases() {
        assert_eq!(KeyStroke::parse("esc").unwrap().key, "escape");
        assert_eq!(KeyStroke::parse("ctrl+return").unwrap().key, "enter");
    }

    #[test]
    fn parses_chord_sequence() {
        let chord = parse_chord("ctrl+k ctrl+s").unwrap();
        assert_eq!(chord.len(), 2);
        assert_eq!(chord[0], s("k").with_ctrl());
        assert_eq!(chord[1], s("s").with_ctrl());
    }

    #[test]
    fn resolves_single_binding() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+s", "file.save");
        assert_eq!(
            km.resolve(&[], &s("s").with_ctrl(), None),
            KeymapOutcome::Execute("file.save".to_string())
        );
        assert_eq!(km.resolve(&[], &s("x").with_ctrl(), None), KeymapOutcome::NoMatch);
    }

    #[test]
    fn chord_pends_then_executes() {
        let mut km = Keymap::new();
        km.bind_default("ctrl+k ctrl+s", "file.save-all");

        let first = s("k").with_ctrl();
        assert_eq!(km.resolve(&[], &first, None), KeymapOutcome::Pending);

        let pending = vec![first];
        assert_eq!(
            km.resolve(&pending, &s("s").with_ctrl(), None),
            KeymapOutcome::Execute("file.save-all".to_string())
        );
        // Wrong continuation cancels the chord.
        assert_eq!(km.resolve(&pending, &s("x").with_ctrl(), None), KeymapOutcome::NoMatch);
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
            km.resolve(&[], &s("p").with_ctrl(), None),
            KeymapOutcome::Execute("command.palette".to_string())
        );
    }

    #[test]
    fn filetype_binding_only_applies_to_matching_filetype() {
        let mut km = Keymap::new();
        km.add_user_config(&[KeymapConfig {
            keys: "ctrl+shift+f".to_string(),
            command: "lsp.format".to_string(),
            filetype: Some("rust".to_string()),
        }]);
        let stroke = s("f").with_ctrl().with_shift();
        assert_eq!(
            km.resolve(&[], &stroke, Some("rust")),
            KeymapOutcome::Execute("lsp.format".to_string())
        );
        assert_eq!(km.resolve(&[], &stroke, Some("toml")), KeymapOutcome::NoMatch);
    }
}
