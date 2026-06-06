//! Platform input → editor input mapping (no editor state, no rendering).
//!
//! Pure translation: aurea key events to logical [`KeyStroke`]s, key codes to
//! characters, modifier snapshots to logical modifier state, and terminal key
//! bytes. The key *router* (which command a stroke runs) lives in `lib.rs`;
//! this module is just the lookup tables.

use ozone_editor::{Key, KeyStroke, ModifierMap, PhysicalMods};

/// Live *logical* modifier state for the status-bar indicator, resolved from the
/// physical keys through the `ModifierMap` (so it matches how bindings read).
#[derive(Clone, Copy, Default)]
pub(crate) struct ActiveMods {
    pub(crate) control: bool,
    pub(crate) meta: bool,
    pub(crate) super_: bool,
    pub(crate) shift: bool,
}

impl ActiveMods {
    pub(crate) fn from_physical(m: aurea::Modifiers, map: &ModifierMap) -> Self {
        // Reuse the keymap's physical→logical resolution.
        let phys = PhysicalMods::new(m.ctrl, m.alt, m.shift, m.meta);
        let ks = KeyStroke::from_physical(phys, Key::Space, map);
        Self { control: ks.control, meta: ks.meta, super_: ks.super_, shift: ks.shift }
    }
    pub(crate) fn any(&self) -> bool {
        self.control || self.meta || self.super_ || self.shift
    }
}

/// Native modifier snapshots are unreliable for a modifier key's *own* press or
/// release (the OS key-state query lags the event), which left the indicator
/// stuck "on". When the event key is itself a modifier, force that bit to match
/// `pressed`; otherwise trust the snapshot.
pub(crate) fn corrected_mods(mut m: aurea::Modifiers, key: aurea::KeyCode, pressed: bool) -> aurea::Modifiers {
    use aurea::KeyCode::*;
    match key {
        Control => m.ctrl = pressed,
        Alt => m.alt = pressed,
        Shift => m.shift = pressed,
        Meta => m.meta = pressed,
        _ => {}
    }
    m
}

/// Map a key + modifiers to the bytes a PTY shell expects, or `None` to let the
/// key fall through to the editor keymap (so Ctrl+Tab, M-x, pane focus still work
/// while a terminal is focused). The shell's line discipline handles echo/editing.
pub(crate) fn terminal_key_bytes(key: aurea::KeyCode, mods: aurea::Modifiers) -> Option<&'static str> {
    use aurea::KeyCode::*;
    if mods.alt {
        return None; // leave Alt (M-x etc.) to the editor
    }
    if mods.ctrl {
        // Only the common control codes go to the shell; other Ctrl combos
        // (Ctrl+Tab, Ctrl+P, …) fall through to the editor.
        return match key {
            C => Some("\u{3}"),  // SIGINT
            D => Some("\u{4}"),  // EOF
            Z => Some("\u{1a}"), // SIGTSTP
            L => Some("\u{c}"),  // clear
            _ => None,
        };
    }
    match key {
        Enter => Some("\r"),
        Backspace => Some("\u{7f}"),
        Tab => Some("\t"),
        Escape => Some("\u{1b}"),
        Up => Some("\u{1b}[A"),
        Down => Some("\u{1b}[B"),
        Right => Some("\u{1b}[C"),
        Left => Some("\u{1b}[D"),
        Home => Some("\u{1b}[H"),
        End => Some("\u{1b}[F"),
        Delete => Some("\u{1b}[3~"),
        _ => None, // printable chars arrive via TextInput
    }
}

/// Convert a platform key + physical modifiers into a logical [`KeyStroke`] via
/// the modifier map. Returns `None` for keys with no token (modifiers, unknown).
pub(crate) fn keystroke_from(key: aurea::KeyCode, mods: aurea::Modifiers, map: &ModifierMap) -> Option<KeyStroke> {
    let k = keycode_key(key)?;
    let phys = PhysicalMods::new(mods.ctrl, mods.alt, mods.shift, mods.meta);
    Some(KeyStroke::from_physical(phys, k, map))
}

/// Map a platform key code to a structured [`Key`]. `None` for modifier-only
/// or unknown codes.
pub(crate) fn keycode_key(key: aurea::KeyCode) -> Option<Key> {
    use aurea::KeyCode::*;
    Some(match key {
        A => Key::Char('a'),
        B => Key::Char('b'),
        C => Key::Char('c'),
        D => Key::Char('d'),
        E => Key::Char('e'),
        F => Key::Char('f'),
        G => Key::Char('g'),
        H => Key::Char('h'),
        I => Key::Char('i'),
        J => Key::Char('j'),
        K => Key::Char('k'),
        L => Key::Char('l'),
        M => Key::Char('m'),
        N => Key::Char('n'),
        O => Key::Char('o'),
        P => Key::Char('p'),
        Q => Key::Char('q'),
        R => Key::Char('r'),
        S => Key::Char('s'),
        T => Key::Char('t'),
        U => Key::Char('u'),
        V => Key::Char('v'),
        W => Key::Char('w'),
        X => Key::Char('x'),
        Y => Key::Char('y'),
        Z => Key::Char('z'),
        Key0 => Key::Char('0'),
        Key1 => Key::Char('1'),
        Key2 => Key::Char('2'),
        Key3 => Key::Char('3'),
        Key4 => Key::Char('4'),
        Key5 => Key::Char('5'),
        Key6 => Key::Char('6'),
        Key7 => Key::Char('7'),
        Key8 => Key::Char('8'),
        Key9 => Key::Char('9'),
        Space => Key::Space,
        Enter => Key::Enter,
        Escape => Key::Escape,
        Tab => Key::Tab,
        Backspace => Key::Backspace,
        Delete => Key::Delete,
        Insert => Key::Insert,
        Home => Key::Home,
        End => Key::End,
        PageUp => Key::PageUp,
        PageDown => Key::PageDown,
        Up => Key::Up,
        Down => Key::Down,
        Left => Key::Left,
        Right => Key::Right,
        F1 => Key::F(1),
        F2 => Key::F(2),
        F3 => Key::F(3),
        F4 => Key::F(4),
        F5 => Key::F(5),
        F6 => Key::F(6),
        F7 => Key::F(7),
        F8 => Key::F(8),
        F9 => Key::F(9),
        F10 => Key::F(10),
        F11 => Key::F(11),
        F12 => Key::F(12),
        // Punctuation / OEM keys, by unshifted character (Shift is a separate modifier).
        Minus => Key::Char('-'),
        Equals => Key::Char('='),
        LeftBracket => Key::Char('['),
        RightBracket => Key::Char(']'),
        Backslash => Key::Char('\\'),
        Semicolon => Key::Char(';'),
        Apostrophe => Key::Char('\''),
        Grave => Key::Char('`'),
        Comma => Key::Char(','),
        Period => Key::Char('.'),
        Slash => Key::Char('/'),
        Shift | Control | Alt | Meta | Unknown(_) => return None,
    })
}

/// Map a key + shift to the character it types (US QWERTY), for text fallback
/// when the platform does not deliver a `TextInput` event.
pub(crate) fn keycode_to_char(key: aurea::KeyCode, shift: bool) -> Option<char> {
    use aurea::KeyCode::*;
    Some(match key {
        A => {
            if shift {
                'A'
            } else {
                'a'
            }
        }
        B => {
            if shift {
                'B'
            } else {
                'b'
            }
        }
        C => {
            if shift {
                'C'
            } else {
                'c'
            }
        }
        D => {
            if shift {
                'D'
            } else {
                'd'
            }
        }
        E => {
            if shift {
                'E'
            } else {
                'e'
            }
        }
        F => {
            if shift {
                'F'
            } else {
                'f'
            }
        }
        G => {
            if shift {
                'G'
            } else {
                'g'
            }
        }
        H => {
            if shift {
                'H'
            } else {
                'h'
            }
        }
        I => {
            if shift {
                'I'
            } else {
                'i'
            }
        }
        J => {
            if shift {
                'J'
            } else {
                'j'
            }
        }
        K => {
            if shift {
                'K'
            } else {
                'k'
            }
        }
        L => {
            if shift {
                'L'
            } else {
                'l'
            }
        }
        M => {
            if shift {
                'M'
            } else {
                'm'
            }
        }
        N => {
            if shift {
                'N'
            } else {
                'n'
            }
        }
        O => {
            if shift {
                'O'
            } else {
                'o'
            }
        }
        P => {
            if shift {
                'P'
            } else {
                'p'
            }
        }
        Q => {
            if shift {
                'Q'
            } else {
                'q'
            }
        }
        R => {
            if shift {
                'R'
            } else {
                'r'
            }
        }
        S => {
            if shift {
                'S'
            } else {
                's'
            }
        }
        T => {
            if shift {
                'T'
            } else {
                't'
            }
        }
        U => {
            if shift {
                'U'
            } else {
                'u'
            }
        }
        V => {
            if shift {
                'V'
            } else {
                'v'
            }
        }
        W => {
            if shift {
                'W'
            } else {
                'w'
            }
        }
        X => {
            if shift {
                'X'
            } else {
                'x'
            }
        }
        Y => {
            if shift {
                'Y'
            } else {
                'y'
            }
        }
        Z => {
            if shift {
                'Z'
            } else {
                'z'
            }
        }
        Key0 => {
            if shift {
                ')'
            } else {
                '0'
            }
        }
        Key1 => {
            if shift {
                '!'
            } else {
                '1'
            }
        }
        Key2 => {
            if shift {
                '@'
            } else {
                '2'
            }
        }
        Key3 => {
            if shift {
                '#'
            } else {
                '3'
            }
        }
        Key4 => {
            if shift {
                '$'
            } else {
                '4'
            }
        }
        Key5 => {
            if shift {
                '%'
            } else {
                '5'
            }
        }
        Key6 => {
            if shift {
                '^'
            } else {
                '6'
            }
        }
        Key7 => {
            if shift {
                '&'
            } else {
                '7'
            }
        }
        Key8 => {
            if shift {
                '*'
            } else {
                '8'
            }
        }
        Key9 => {
            if shift {
                '('
            } else {
                '9'
            }
        }
        Space => ' ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_events_override_stale_native_snapshot() {
        let mods = aurea::Modifiers { ctrl: false, alt: false, shift: false, meta: false };

        assert!(corrected_mods(mods, aurea::KeyCode::Control, true).ctrl);

        let held = aurea::Modifiers { ctrl: true, ..mods };
        assert!(!corrected_mods(held, aurea::KeyCode::Control, false).ctrl);
    }

    #[test]
    fn terminal_keys_preserve_editor_shortcuts() {
        let control = aurea::Modifiers { ctrl: true, alt: false, shift: false, meta: false };

        assert_eq!(terminal_key_bytes(aurea::KeyCode::C, control), Some("\u{3}"));
        assert_eq!(terminal_key_bytes(aurea::KeyCode::P, control), None);
    }

    #[test]
    fn shifted_character_fallback_uses_us_layout() {
        assert_eq!(keycode_to_char(aurea::KeyCode::A, true), Some('A'));
        assert_eq!(keycode_to_char(aurea::KeyCode::Key1, true), Some('!'));
        assert_eq!(keycode_to_char(aurea::KeyCode::Enter, false), None);
    }
}
