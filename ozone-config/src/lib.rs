//! TOML-based configuration — Phase 0 provides defaults only.
//!
//! Phase 1 will add full parsing and file-watching.

use std::path::PathBuf;

/// Global editor settings (mirrors config.toml `[editor]`).
#[derive(Debug, Clone)]
pub struct EditorConfig {
    pub font: String,
    pub font_size: f32,
    pub line_height: f32,
    pub tab_width: usize,
    pub soft_tabs: bool,
    pub line_numbers: LineNumbers,
    pub scroll_off: usize,
    pub word_wrap: bool,
    pub trim_trailing_whitespace: bool,
    pub auto_save: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineNumbers {
    Off,
    Absolute,
    Relative,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            font: "Consolas".to_string(),
            font_size: 14.0,
            line_height: 1.4,
            tab_width: 4,
            soft_tabs: true,
            line_numbers: LineNumbers::Absolute,
            scroll_off: 8,
            word_wrap: false,
            trim_trailing_whitespace: true,
            auto_save: false,
        }
    }
}

/// Top-level configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub editor: EditorConfig,
    pub theme: String,
}

impl Config {
    pub fn default_config() -> Self {
        Self {
            editor: EditorConfig::default(),
            theme: "catppuccin-mocha".to_string(),
        }
    }

    /// Load from a TOML file, falling back to defaults on error.
    pub fn load(path: &PathBuf) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default_config();
        };
        // Full TOML parsing comes in Phase 1
        let _ = text;
        Self::default_config()
    }
}
