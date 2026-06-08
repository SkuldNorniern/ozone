use super::keys::{ModifierMap, PhysicalMods};

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
                if let Some(num) = t.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
                    && (1..=12).contains(&num)
                {
                    return Some(Key::F(num));
                }
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
        Self {
            control: false,
            meta: false,
            super_: false,
            shift: false,
            key,
        }
    }

    pub fn with_control(mut self) -> Self {
        self.control = true;
        self
    }
    pub fn with_meta(mut self) -> Self {
        self.meta = true;
        self
    }
    pub fn with_super(mut self) -> Self {
        self.super_ = true;
        self
    }
    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

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

    /// Parse one stroke token like `"ctrl+shift+f"` or `"meta+x"`.
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
        Some(Self {
            control,
            meta,
            super_,
            shift,
            key: key?,
        })
    }
}

/// Human-readable label for a single stroke (`"C-x"`, `"M-g"`, `"Enter"`).
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

/// Human-readable label for a full chord (`"C-K C-S"`).
pub fn chord_label(chord: &[KeyStroke]) -> String {
    chord.iter().map(stroke_label).collect::<Vec<_>>().join(" ")
}

/// Parse a full chord string like `"ctrl+k ctrl+s"` into its strokes.
pub fn parse_chord(keys: &str) -> Option<Vec<KeyStroke>> {
    let strokes: Vec<KeyStroke> = keys
        .split_whitespace()
        .filter_map(KeyStroke::parse)
        .collect();
    if strokes.is_empty() {
        None
    } else {
        Some(strokes)
    }
}
