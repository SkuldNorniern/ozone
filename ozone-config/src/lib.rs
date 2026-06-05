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
    pub auto_format: bool,
    pub jump_list_size: usize,
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
            auto_format: false,
            jump_list_size: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapConfig {
    pub keys: String,
    pub command: String,
    pub filetype: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocmdConfig {
    pub event: String,
    pub pattern: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FiletypeConfig {
    pub name: String,
    pub tab_width: Option<usize>,
    pub soft_tabs: Option<bool>,
    pub line_numbers: Option<LineNumbers>,
    pub word_wrap: Option<bool>,
    pub trim_trailing_whitespace: Option<bool>,
    pub auto_format: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LspCapabilities {
    pub completion: bool,
    pub diagnostics: bool,
    pub hover: bool,
    pub goto_definition: bool,
    pub find_references: bool,
    pub rename: bool,
    pub format: bool,
    pub code_actions: bool,
    pub inlay_hints: bool,
    pub semantic_tokens: bool,
    pub code_lens: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspConfig {
    pub language: String,
    pub server: String,
    pub args: Vec<String>,
    pub lazy: bool,
    pub capabilities: LspCapabilities,
}

/// Optional `[modifiers]` overrides: which physical key each Emacs-style logical
/// modifier maps to. `None` = use the platform default. Values are physical
/// modifier tokens: `ctrl`, `alt`, `shift`, `meta`/`super`/`cmd`/`win`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModifierOverrides {
    pub control: Option<String>,
    pub meta: Option<String>,
    pub super_: Option<String>,
}

/// Top-level configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub editor: EditorConfig,
    pub theme: String,
    pub keymaps: Vec<KeymapConfig>,
    pub autocmds: Vec<AutocmdConfig>,
    pub filetypes: Vec<FiletypeConfig>,
    pub lsps: Vec<LspConfig>,
    pub modifiers: ModifierOverrides,
}

impl Config {
    pub fn default_config() -> Self {
        Self {
            editor: EditorConfig::default(),
            theme: "catppuccin-mocha".to_string(),
            keymaps: Vec::new(),
            autocmds: Vec::new(),
            filetypes: Vec::new(),
            lsps: Vec::new(),
            modifiers: ModifierOverrides::default(),
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
            if let Some(v) = editor.get("auto_format").and_then(|v| v.as_bool()) {
                e.auto_format = v;
            }
            if let Some(v) = as_usize(editor.get("jump_list_size")) {
                e.jump_list_size = v.max(1);
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

        config.keymaps = parse_keymaps(&table);
        config.autocmds = parse_autocmds(&table);
        config.filetypes = parse_filetypes(&table);
        config.lsps = parse_lsps(&table);
        config.modifiers = parse_modifiers(&table);

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

fn table_array<'a>(table: &'a toml::Table, key: &str) -> impl Iterator<Item = &'a toml::Table> {
    table
        .get(key)
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|items| items.iter())
        .filter_map(|item| item.as_table())
}

fn non_empty_string(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn string_array(table: &toml::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|items| items.iter())
        .filter_map(|item| item.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_keymaps(table: &toml::Table) -> Vec<KeymapConfig> {
    table_array(table, "keymap")
        .filter_map(|entry| {
            Some(KeymapConfig {
                keys: non_empty_string(entry, "keys")?,
                command: non_empty_string(entry, "command")?,
                filetype: non_empty_string(entry, "filetype"),
            })
        })
        .collect()
}

fn parse_autocmds(table: &toml::Table) -> Vec<AutocmdConfig> {
    table_array(table, "autocmd")
        .filter_map(|entry| {
            Some(AutocmdConfig {
                event: non_empty_string(entry, "event")?,
                pattern: non_empty_string(entry, "pattern").unwrap_or_else(|| "*".to_string()),
                command: non_empty_string(entry, "command")?,
            })
        })
        .collect()
}

fn parse_filetypes(table: &toml::Table) -> Vec<FiletypeConfig> {
    table_array(table, "filetype")
        .filter_map(|entry| {
            Some(FiletypeConfig {
                name: non_empty_string(entry, "name")?,
                tab_width: as_usize(entry.get("tab_width")).map(|v| v.max(1)),
                soft_tabs: entry.get("soft_tabs").and_then(|v| v.as_bool()),
                line_numbers: entry
                    .get("line_numbers")
                    .and_then(|v| v.as_str())
                    .and_then(LineNumbers::parse),
                word_wrap: entry.get("word_wrap").and_then(|v| v.as_bool()),
                trim_trailing_whitespace: entry
                    .get("trim_trailing_whitespace")
                    .and_then(|v| v.as_bool()),
                auto_format: entry.get("auto_format").and_then(|v| v.as_bool()),
            })
        })
        .collect()
}

fn parse_lsps(table: &toml::Table) -> Vec<LspConfig> {
    table_array(table, "lsp")
        .filter_map(|entry| {
            Some(LspConfig {
                language: non_empty_string(entry, "language")?,
                server: non_empty_string(entry, "server")?,
                args: string_array(entry, "args"),
                lazy: entry.get("lazy").and_then(|v| v.as_bool()).unwrap_or(true),
                capabilities: parse_lsp_capabilities(entry),
            })
        })
        .collect()
}

fn parse_lsp_capabilities(entry: &toml::Table) -> LspCapabilities {
    let Some(caps) = entry.get("capabilities").and_then(|v| v.as_table()) else {
        return LspCapabilities::default();
    };

    LspCapabilities {
        completion: bool_field(caps, "completion"),
        diagnostics: bool_field(caps, "diagnostics"),
        hover: bool_field(caps, "hover"),
        goto_definition: bool_field(caps, "goto_definition"),
        find_references: bool_field(caps, "find_references"),
        rename: bool_field(caps, "rename"),
        format: bool_field(caps, "format"),
        code_actions: bool_field(caps, "code_actions"),
        inlay_hints: bool_field(caps, "inlay_hints"),
        semantic_tokens: bool_field(caps, "semantic_tokens"),
        code_lens: bool_field(caps, "code_lens"),
    }
}

fn bool_field(table: &toml::Table, key: &str) -> bool {
    table.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Parse the optional `[modifiers]` table.
fn parse_modifiers(table: &toml::Table) -> ModifierOverrides {
    let Some(m) = table.get("modifiers").and_then(|v| v.as_table()) else {
        return ModifierOverrides::default();
    };
    let get = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
    ModifierOverrides {
        control: get("control"),
        meta: get("meta"),
        // accept either `super` or `super_`
        super_: get("super").or_else(|| get("super_")),
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
            auto_format = true
            jump_list_size = 42

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
        assert!(c.editor.auto_format);
        assert_eq!(c.editor.jump_list_size, 42);
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

    #[test]
    fn parses_keymap_autocmd_filetype_and_lsp_blocks() {
        let c = Config::parse_str(
            r#"
            [[keymap]]
            keys = "ctrl+s"
            command = "file.save"

            [[keymap]]
            keys = "ctrl+shift+f"
            command = "lsp.format"
            filetype = "rust"

            [[autocmd]]
            event = "buffer.pre-save"
            pattern = "rust"
            command = "lsp.format"

            [[filetype]]
            name = "markdown"
            word_wrap = true
            tab_width = 2
            line_numbers = "off"

            [[lsp]]
            language = "rust"
            server = "rust-analyzer"
            args = ["--stdio"]
            lazy = true
            [lsp.capabilities]
            completion = true
            diagnostics = true
            semantic_tokens = false
        "#,
        );

        assert_eq!(c.keymaps.len(), 2);
        assert_eq!(c.keymaps[0].keys, "ctrl+s");
        assert_eq!(c.keymaps[1].filetype.as_deref(), Some("rust"));

        assert_eq!(c.autocmds.len(), 1);
        assert_eq!(c.autocmds[0].event, "buffer.pre-save");

        assert_eq!(c.filetypes.len(), 1);
        assert_eq!(c.filetypes[0].name, "markdown");
        assert_eq!(c.filetypes[0].word_wrap, Some(true));
        assert_eq!(c.filetypes[0].tab_width, Some(2));
        assert_eq!(c.filetypes[0].line_numbers, Some(LineNumbers::Off));

        assert_eq!(c.lsps.len(), 1);
        assert_eq!(c.lsps[0].language, "rust");
        assert_eq!(c.lsps[0].server, "rust-analyzer");
        assert_eq!(c.lsps[0].args, vec!["--stdio"]);
        assert!(c.lsps[0].lazy);
        assert!(c.lsps[0].capabilities.completion);
        assert!(c.lsps[0].capabilities.diagnostics);
        assert!(!c.lsps[0].capabilities.semantic_tokens);
    }

    #[test]
    fn malformed_array_entries_are_ignored() {
        let c = Config::parse_str(
            r#"
            [[keymap]]
            keys = "ctrl+s"

            [[keymap]]
            keys = "ctrl+z"
            command = "edit.undo"

            [[filetype]]
            tab_width = 2

            [[lsp]]
            language = "rust"
        "#,
        );

        assert_eq!(c.keymaps.len(), 1);
        assert_eq!(c.keymaps[0].command, "edit.undo");
        assert!(c.filetypes.is_empty());
        assert!(c.lsps.is_empty());
    }
}
