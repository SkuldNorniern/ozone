//! TOML-based configuration.
//!
//! Parsing is intentionally manual: per the dependency policy in PLAN.md, Ozone
//! does not expose a Serde-derived domain model. We parse into a `toml::Table`
//! and pull fields out by hand, defaulting anything missing or malformed. A bad
//! config never crashes the editor — it degrades to defaults field by field.

use std::path::{Path, PathBuf};

/// Global editor settings (mirrors config.toml `[editor]`).
#[derive(Debug, Clone)]
pub struct EditorConfig {
    pub font: String,
    pub font_size: f32,
    pub line_height: f32,
    pub tab_width: usize,
    pub soft_tabs: bool,
    pub line_numbers: LineNumbers,
    pub cursor_style: CursorStyle,
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

impl LineNumbers {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" => Some(Self::Off),
            "absolute" | "on" | "true" => Some(Self::Absolute),
            "relative" => Some(Self::Relative),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

impl CursorStyle {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "block" => Some(Self::Block),
            "bar" | "line" => Some(Self::Bar),
            "underline" => Some(Self::Underline),
            _ => None,
        }
    }
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
            cursor_style: CursorStyle::Bar,
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

    /// Parse a TOML config string, falling back to defaults per missing field.
    pub fn parse_str(text: &str) -> Self {
        let mut config = Self::default_config();
        let Ok(table) = text.parse::<toml::Table>() else {
            return config;
        };

        if let Some(editor) = table.get("editor").and_then(|v| v.as_table()) {
            let e = &mut config.editor;
            if let Some(v) = editor.get("font").and_then(|v| v.as_str()) {
                if !v.trim().is_empty() {
                    e.font = v.to_string();
                }
            }
            if let Some(v) = as_f32(editor.get("font_size")) {
                if v > 0.0 {
                    e.font_size = v;
                }
            }
            if let Some(v) = as_f32(editor.get("line_height")) {
                if v > 0.0 {
                    e.line_height = v;
                }
            }
            if let Some(v) = as_usize(editor.get("tab_width")) {
                e.tab_width = v.max(1);
            }
            if let Some(v) = editor.get("soft_tabs").and_then(|v| v.as_bool()) {
                e.soft_tabs = v;
            }
            if let Some(v) = editor
                .get("line_numbers")
                .and_then(|v| v.as_str())
                .and_then(LineNumbers::parse)
            {
                e.line_numbers = v;
            }
            if let Some(v) = editor
                .get("cursor_style")
                .and_then(|v| v.as_str())
                .and_then(CursorStyle::parse)
            {
                e.cursor_style = v;
            }
            if let Some(v) = as_usize(editor.get("scroll_off")) {
                e.scroll_off = v;
            }
            if let Some(v) = editor.get("word_wrap").and_then(|v| v.as_bool()) {
                e.word_wrap = v;
            }
            if let Some(v) = editor
                .get("trim_trailing_whitespace")
                .and_then(|v| v.as_bool())
            {
                e.trim_trailing_whitespace = v;
            }
            if let Some(v) = editor.get("auto_save").and_then(|v| v.as_bool()) {
                e.auto_save = v;
            }
        }

        if let Some(name) = table
            .get("theme")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
        {
            if !name.trim().is_empty() {
                config.theme = name.to_string();
            }
        }

        config
    }

    /// Load from a TOML file, falling back to defaults if it cannot be read.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse_str(&text),
            Err(_) => Self::default_config(),
        }
    }

    /// The platform user config path: `%APPDATA%\ozone\config.toml` on Windows,
    /// `$XDG_CONFIG_HOME/ozone/config.toml` (or `~/.config/...`) elsewhere.
    pub fn user_config_path() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        }?;
        Some(base.join("ozone").join("config.toml"))
    }

    /// Load the user config if present, otherwise defaults.
    pub fn load_user() -> Self {
        match Self::user_config_path() {
            Some(path) if path.exists() => Self::load(&path),
            _ => Self::default_config(),
        }
    }
}

/// Coerce a TOML value (integer or float) into `f32`.
fn as_f32(value: Option<&toml::Value>) -> Option<f32> {
    match value {
        Some(toml::Value::Float(f)) => Some(*f as f32),
        Some(toml::Value::Integer(i)) => Some(*i as f32),
        _ => None,
    }
}

/// Coerce a non-negative TOML integer into `usize`.
fn as_usize(value: Option<&toml::Value>) -> Option<usize> {
    match value {
        Some(toml::Value::Integer(i)) if *i >= 0 => Some(*i as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_yields_defaults() {
        let c = Config::parse_str("");
        assert_eq!(c.editor.font_size, 14.0);
        assert_eq!(c.theme, "catppuccin-mocha");
    }

    #[test]
    fn parses_editor_block() {
        let c = Config::parse_str(
            r#"
            [editor]
            font = "JetBrains Mono"
            font_size = 10
            line_height = 1.6
            tab_width = 2
            line_numbers = "relative"
            cursor_style = "block"

            [theme]
            name = "gruvbox"
        "#,
        );
        assert_eq!(c.editor.font, "JetBrains Mono");
        assert_eq!(c.editor.font_size, 10.0);
        assert_eq!(c.editor.line_height, 1.6);
        assert_eq!(c.editor.tab_width, 2);
        assert_eq!(c.editor.line_numbers, LineNumbers::Relative);
        assert_eq!(c.editor.cursor_style, CursorStyle::Block);
        assert_eq!(c.theme, "gruvbox");
    }

    #[test]
    fn malformed_toml_falls_back() {
        let c = Config::parse_str("this is = = not valid toml [[[");
        assert_eq!(c.editor.font_size, 14.0);
    }

    #[test]
    fn partial_block_keeps_defaults_for_missing() {
        let c = Config::parse_str("[editor]\nfont_size = 12\n");
        assert_eq!(c.editor.font_size, 12.0);
        assert_eq!(c.editor.tab_width, 4); // default preserved
    }
}
