//! TOML-based configuration.
//!
//! Parsing is intentionally manual: per the dependency policy in PLAN.md, Ozone
//! does not expose a Serde-derived domain model. We parse into a `toml::Table`
//! and pull fields out by hand, defaulting anything missing or malformed. A bad
//! config never crashes the editor — it degrades to defaults field by field.

mod parse;

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

/// Frontend behavior toggles (mirrors config.toml `[ui]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UiConfig {
    /// Enables mouse-driven editor interaction. Keyboard input remains active
    /// regardless of this setting.
    pub mouse: bool,
}

/// Top-level configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub editor: EditorConfig,
    /// Theme name or path. Names resolve through the user and bundled theme
    /// directories; paths load a specific TOML file.
    pub theme: String,
    pub ui: UiConfig,
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
            theme: "brewery-stout".to_string(),
            ui: UiConfig::default(),
            keymaps: Vec::new(),
            autocmds: Vec::new(),
            filetypes: Vec::new(),
            lsps: Vec::new(),
            modifiers: ModifierOverrides::default(),
        }
    }

    /// Parse a TOML config string, falling back to defaults per missing field.
    pub fn parse_str(text: &str) -> Self {
        Self::parse_str_result(text).unwrap_or_else(|_| Self::default_config())
    }

    /// Parse a TOML config string, returning TOML syntax errors to callers that
    /// want to surface diagnostics.
    pub fn parse_str_result(text: &str) -> Result<Self, toml::de::Error> {
        let mut config = Self::default_config();
        let table = text.parse::<toml::Table>()?;

        if let Some(editor) = table.get("editor").and_then(|v| v.as_table()) {
            let e = &mut config.editor;
            if let Some(v) = editor.get("font").and_then(|v| v.as_str())
                && !v.trim().is_empty()
            {
                e.font = v.to_string();
            }
            if let Some(v) = parse::as_f32(editor.get("font_size"))
                && v > 0.0
            {
                e.font_size = v;
            }
            if let Some(v) = parse::as_f32(editor.get("line_height"))
                && v > 0.0
            {
                e.line_height = v;
            }
            if let Some(v) = parse::as_usize(editor.get("tab_width")) {
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
            if let Some(v) = parse::as_usize(editor.get("scroll_off")) {
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
            if let Some(v) = parse::as_usize(editor.get("jump_list_size")) {
                e.jump_list_size = v.max(1);
            }
        }

        if let Some(name) = table
            .get("theme")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            && !name.trim().is_empty()
        {
            config.theme = name.to_string();
        }

        if let Some(ui) = table.get("ui").and_then(|v| v.as_table())
            && let Some(mouse) = ui.get("mouse").and_then(|v| v.as_bool())
        {
            config.ui.mouse = mouse;
        }

        config.keymaps = parse::parse_keymaps(&table);
        config.autocmds = parse::parse_autocmds(&table);
        config.filetypes = parse::parse_filetypes(&table);
        config.lsps = parse::parse_lsps(&table);
        config.modifiers = parse::parse_modifiers(&table);

        Ok(config)
    }

    /// Load from a TOML file, falling back to defaults if it cannot be read.
    pub fn load(path: &Path) -> Self {
        Self::load_with_warning(path).0
    }

    /// Load from a TOML file, returning a warning if the file had to be ignored.
    pub fn load_with_warning(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse_str_result(&text) {
                Ok(config) => (config, None),
                Err(error) => (
                    Self::default_config(),
                    Some(format!(
                        "could not parse config {}: {error}",
                        path.display()
                    )),
                ),
            },
            Err(error) => (
                Self::default_config(),
                Some(format!("could not read config {}: {error}", path.display())),
            ),
        }
    }

    /// Resolve the config path that [`Self::load_user`] would use.
    pub fn resolved_config_path() -> Option<PathBuf> {
        if let Some(path) = Self::user_config_path()
            && path.exists()
        {
            return Some(path);
        }
        let local = PathBuf::from("config.toml");
        local.exists().then_some(local)
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

    /// Load the user config if present, otherwise `./config.toml` if present,
    /// otherwise defaults. The local fallback keeps `cargo run` from the repo
    /// root aligned with the checked-in reference config.
    pub fn load_user() -> Self {
        Self::load_user_with_warning().0
    }

    /// Load the user/local config, plus a warning when that file is unreadable
    /// or syntactically invalid.  When no config exists anywhere, write the
    /// default template to the user config path so the user has a file to edit.
    pub fn load_user_with_warning() -> (Self, Option<String>) {
        if let Some(path) = Self::resolved_config_path() {
            return Self::load_with_warning(&path);
        }

        // No config found – generate one at the user path.
        if let Some(path) = Self::user_config_path() {
            let _ = Self::write_default_to(&path);
        }

        (Self::default_config(), None)
    }

    /// Write the built-in default config template to `path`, creating parent
    /// directories as needed.  Returns an error string on failure.
    pub fn write_default_to(path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create config dir: {e}"))?;
        }
        std::fs::write(path, Self::DEFAULT_TEMPLATE)
            .map_err(|e| format!("write config: {e}"))
    }

    /// The default configuration template written when no user config exists.
    pub const DEFAULT_TEMPLATE: &'static str = include_str!("../../config.toml");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_yields_defaults() {
        let c = Config::parse_str("");
        assert_eq!(c.editor.font_size, 14.0);
        assert_eq!(c.theme, "brewery-stout");
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

            [ui]
            mouse = true
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
        assert!(c.ui.mouse);
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
    fn load_uses_requested_font_from_file() {
        let dir = std::env::temp_dir().join(format!("ozone-config-font-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[editor]\nfont = \"Cascadia Mono\"\nfont_size = 15\n",
        )
        .unwrap();

        let c = Config::load(&path);
        assert_eq!(c.editor.font, "Cascadia Mono");
        assert_eq!(c.editor.font_size, 15.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_reports_malformed_config() {
        let dir =
            std::env::temp_dir().join(format!("ozone-config-malformed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[theme]\nname = \"name = \"brewery-stout\"\n").unwrap();

        let (c, warning) = Config::load_with_warning(&path);
        assert_eq!(c.editor.font, EditorConfig::default().font);
        assert!(warning.is_some_and(|w| w.contains("could not parse config")));

        let _ = std::fs::remove_dir_all(&dir);
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
