//! Buffer-local options (Neovim `vim.bo` / Emacs buffer-local variables).
//!
//! Each buffer may override a handful of editor settings that otherwise come
//! from the global `[editor]` config. Overrides are set by `[[filetype]]` rules
//! (applied on open), by autocommands, or by plugins via
//! [`crate::EditorApi::set_local`]. Consumers read the *effective* value:
//! buffer-local if present, else the global default.

use ozone_config::LineNumbers;

/// A value passed to a string-keyed option setter (config / plugin path).
#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    Str(String),
}

impl OptionValue {
    fn as_bool(&self) -> Option<bool> {
        match self {
            OptionValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn as_usize(&self) -> Option<usize> {
        match self {
            OptionValue::Int(n) if *n >= 0 => Some(*n as usize),
            _ => None,
        }
    }
}

/// Per-buffer setting overrides. `None` means "inherit the global default".
#[derive(Debug, Clone, Default)]
pub struct BufferLocal {
    pub tab_width: Option<usize>,
    pub soft_tabs: Option<bool>,
    pub line_numbers: Option<LineNumbers>,
    pub word_wrap: Option<bool>,
}

impl BufferLocal {
    /// Set an option by name (the config/plugin surface). Unknown keys or
    /// type-mismatched values are ignored.
    pub fn set(&mut self, key: &str, value: OptionValue) {
        match key {
            "tab_width" => self.tab_width = value.as_usize().filter(|w| *w > 0),
            "soft_tabs" => self.soft_tabs = value.as_bool(),
            "word_wrap" => self.word_wrap = value.as_bool(),
            "line_numbers" => {
                if let OptionValue::Str(s) = value {
                    self.line_numbers = match s.as_str() {
                        "off" => Some(LineNumbers::Off),
                        "absolute" => Some(LineNumbers::Absolute),
                        "relative" => Some(LineNumbers::Relative),
                        _ => self.line_numbers,
                    };
                }
            }
            _ => {}
        }
    }
}
