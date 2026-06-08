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
        Self {
            ctrl,
            alt,
            shift,
            meta,
        }
    }

    pub(super) fn has(&self, m: PhysicalModifier) -> bool {
        match m {
            PhysicalModifier::Ctrl => self.ctrl,
            PhysicalModifier::Alt => self.alt,
            PhysicalModifier::Shift => self.shift,
            PhysicalModifier::Meta => self.meta,
        }
    }
}

pub(super) fn parse_physical_modifier(s: &str) -> Option<PhysicalModifier> {
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
/// Defaults: Control→Ctrl, Meta→Alt, Super→the OS Win/Cmd key.
#[derive(Debug, Clone, Copy)]
pub struct ModifierMap {
    pub control: PhysicalModifier,
    pub meta: PhysicalModifier,
    pub super_: PhysicalModifier,
}

impl ModifierMap {
    pub fn platform_default() -> Self {
        Self {
            control: PhysicalModifier::Ctrl,
            meta: PhysicalModifier::Alt,
            super_: PhysicalModifier::Meta,
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
