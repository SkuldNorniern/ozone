//! TOML-based configuration.
//!
//! Parsing is intentionally manual: per the dependency policy in PLAN.md, Ozone
//! does not expose a Serde-derived domain model. We parse into a `toml::Table`
//! and pull fields out by hand, defaulting anything missing or malformed. A bad
//! config never crashes the editor — it degrades to defaults field by field.

mod parse;

use std::path::{Path, PathBuf};

/// Merges one per-section file's parsed table into a [`Config`] (appends its
/// entries). See [`Config::SECTION_FILES`].
type SectionMerge = fn(&mut Config, &toml::Table, &mut Vec<String>);

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
    /// If set, binding only applies on the named OS: `"macos"`, `"windows"`, `"linux"`.
    pub platform: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

impl Default for LspCapabilities {
    /// Enable the common IDE features by default; the heavier/rarer ones
    /// (inlay hints, semantic tokens, code lens) are opt-in. So a `[[lsp]]`
    /// block needs no `[lsp.capabilities]` table unless you want to turn
    /// something off.
    fn default() -> Self {
        Self {
            completion: true,
            diagnostics: true,
            hover: true,
            goto_definition: true,
            find_references: true,
            rename: true,
            format: true,
            code_actions: true,
            inlay_hints: false,
            semantic_tokens: false,
            code_lens: false,
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiConfig {
    /// Enables mouse-driven editor interaction. Keyboard input remains active
    /// regardless of this setting.
    pub mouse: bool,
    /// Draw vertical lines at each indentation level in text buffers.
    pub indent_guides: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            mouse: false,
            indent_guides: true,
        }
    }
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
    /// Non-fatal problems found while parsing this config (malformed
    /// `[[autocmd]]` / `[[filetype]]` / `[[lsp]]` entries that were skipped).
    /// Surfaced by [`Self::load_with_warning`] so a bad entry is visible
    /// instead of silently vanishing. Not part of the user's configuration.
    pub parse_warnings: Vec<String>,
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
            parse_warnings: Vec::new(),
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

        if let Some(ui) = table.get("ui").and_then(|v| v.as_table()) {
            if let Some(mouse) = ui.get("mouse").and_then(|v| v.as_bool()) {
                config.ui.mouse = mouse;
            }
            if let Some(v) = ui.get("indent_guides").and_then(|v| v.as_bool()) {
                config.ui.indent_guides = v;
            }
        }

        let mut warnings = Vec::new();
        config.keymaps = parse::parse_keymaps(&table);
        config.autocmds = parse::parse_autocmds(&table, &mut warnings);
        config.filetypes = parse::parse_filetypes(&table, &mut warnings);
        config.lsps = parse::parse_lsps(&table, &mut warnings);
        config.modifiers = parse::parse_modifiers(&table);
        config.parse_warnings = warnings;

        Ok(config)
    }

    /// Load from a TOML file, falling back to defaults if it cannot be read.
    pub fn load(path: &Path) -> Self {
        Self::load_with_warning(path).0
    }

    /// Per-section files that, when present alongside `config.toml`, **own** that
    /// section. Each holds the same block syntax it would in `config.toml` (e.g.
    /// `keymap.toml` has `[keymap]` / `[[keymap]]` blocks). If a section file
    /// exists, the matching section in `config.toml` is ignored — one concern,
    /// one place — so a config directory can be split:
    ///
    /// ```text
    /// ~/.config/ozone/
    ///   config.toml      # [editor], [theme], [ui], …
    ///   keymap.toml      # [keymap] / [[keymap]]
    ///   autocmd.toml     # [[autocmd]]
    ///   filetype.toml    # [[filetype]]
    ///   lsp.toml         # [[lsp]]
    /// ```
    const SECTION_FILES: &'static [(&'static str, SectionMerge)] = &[
        ("keymap.toml", |c, t, _w| {
            c.keymaps = parse::parse_keymaps(t)
        }),
        ("autocmd.toml", |c, t, w| {
            c.autocmds = parse::parse_autocmds(t, w)
        }),
        ("filetype.toml", |c, t, w| {
            c.filetypes = parse::parse_filetypes(t, w)
        }),
        ("lsp.toml", |c, t, w| c.lsps = parse::parse_lsps(t, w)),
    ];

    /// Default content for each section file, written on first run so the
    /// generated config starts already split.
    const SECTION_TEMPLATES: &'static [(&'static str, &'static str)] = &[
        ("keymap.toml", include_str!("../../keymap.toml")),
        ("autocmd.toml", include_str!("../../autocmd.toml")),
        ("filetype.toml", include_str!("../../filetype.toml")),
        ("lsp.toml", include_str!("../../lsp.toml")),
    ];

    /// Load from a TOML file, returning a warning if the file had to be ignored.
    /// Sibling per-section files in the same directory ([`Self::SECTION_FILES`])
    /// are merged on top.
    pub fn load_with_warning(path: &Path) -> (Self, Option<String>) {
        let mut warnings: Vec<String> = Vec::new();

        let mut config = match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse_str_result(&text) {
                Ok(config) => config,
                Err(error) => {
                    warnings.push(format!(
                        "could not parse config {}: {error}",
                        path.display()
                    ));
                    Self::default_config()
                }
            },
            Err(error) => {
                warnings.push(format!("could not read config {}: {error}", path.display()));
                Self::default_config()
            }
        };

        // Surface malformed-entry warnings from config.toml itself; section
        // files report their own (prefixed) warnings inside merge_section_files.
        for w in std::mem::take(&mut config.parse_warnings) {
            warnings.push(format!("{}: {w}", path.display()));
        }

        config.merge_section_files(path.parent(), &mut warnings);

        // Bindings are config-driven with no hardcoded fallback layer: a config
        // without any `[keymap]`/`[[keymap]]` (typically one written before the
        // keymap.toml split) leaves *every* key — including Ctrl/Meta chords —
        // unbound. Surface that loudly rather than silently rewriting the
        // user's config directory.
        if config.keymaps.is_empty() {
            warnings.push(format!(
                "no keybindings configured in {} (or its keymap.toml) — Ctrl/Meta \
                 shortcuts are unbound; run `ozone --reset-config` to regenerate the \
                 default keymap",
                path.display()
            ));
        }

        let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
        (config, warning)
    }

    /// Merge any per-section files found in `dir` into this config, pushing a
    /// warning for each file that exists but can't be read/parsed.
    fn merge_section_files(&mut self, dir: Option<&Path>, warnings: &mut Vec<String>) {
        let Some(dir) = dir else { return };
        for (file, merge) in Self::SECTION_FILES {
            let p = dir.join(file);
            if !p.exists() {
                continue;
            }
            match std::fs::read_to_string(&p) {
                Ok(text) => match text.parse::<toml::Table>() {
                    Ok(table) => {
                        let before = warnings.len();
                        merge(self, &table, warnings);
                        // Prefix any malformed-entry warnings with the file they
                        // came from, so the message points at the right place.
                        for w in &mut warnings[before..] {
                            *w = format!("{}: {w}", p.display());
                        }
                    }
                    Err(e) => warnings.push(format!("could not parse {}: {e}", p.display())),
                },
                Err(e) => warnings.push(format!("could not read {}: {e}", p.display())),
            }
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

        // No config found – generate the split layout at the user path, then
        // load it so this first session already has the generated keymap/etc.
        // (Bindings are config-driven, not hardcoded, so they must be loaded.)
        if let Some(path) = Self::user_config_path()
            && Self::write_default_to(&path).is_ok()
        {
            return Self::load_with_warning(&path);
        }

        (Self::default_config(), None)
    }

    /// Write any of [`Self::SECTION_TEMPLATES`] not already present in `dir`.
    /// Existing section files are left untouched. Creates `dir` if needed.
    fn write_missing_section_files(dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create config dir: {e}"))?;
        for (name, template) in Self::SECTION_TEMPLATES {
            let section = dir.join(name);
            if !section.exists() {
                std::fs::write(&section, template)
                    .map_err(|e| format!("write {}: {e}", section.display()))?;
            }
        }
        Ok(())
    }

    /// Write the default config to `path`, generating the **split layout**:
    /// the base `config.toml` plus a sibling file per section (`keymap.toml`,
    /// `autocmd.toml`, `filetype.toml`, `lsp.toml`). Creates parent directories
    /// as needed. Existing section files are never overwritten; the base
    /// `config.toml` is written at `path`.
    pub fn write_default_to(path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            Self::write_missing_section_files(parent)?;
        }
        std::fs::write(path, Self::DEFAULT_TEMPLATE).map_err(|e| format!("write config: {e}"))
    }

    /// Force-regenerate the user config's split layout: `config.toml` plus
    /// every section file (`keymap.toml`, `autocmd.toml`, `filetype.toml`,
    /// `lsp.toml`) are overwritten with the shipped defaults, even if they
    /// already exist. Unlike [`Self::write_default_to`], this is destructive —
    /// it is the explicit `--reset-config` escape hatch for a config that
    /// predates the split layout (and so has no keybindings at all).
    /// Returns the path written to.
    pub fn reset_user_config() -> Result<PathBuf, String> {
        let path = Self::user_config_path().ok_or("cannot determine user config directory")?;
        Self::reset_at(&path)?;
        Ok(path)
    }

    /// Implementation of [`Self::reset_user_config`], taking an explicit path
    /// so it can be exercised against a temp directory in tests.
    fn reset_at(path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
            for (name, template) in Self::SECTION_TEMPLATES {
                std::fs::write(parent.join(name), template)
                    .map_err(|e| format!("write {name}: {e}"))?;
            }
        }
        std::fs::write(path, Self::DEFAULT_TEMPLATE).map_err(|e| format!("write config: {e}"))
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
    fn malformed_list_entries_are_reported_not_silently_dropped() {
        let c = Config::parse_str(
            r#"
            [[autocmd]]
            pattern = "rust"
            command = "lsp.format"   # missing event

            [[autocmd]]
            event = "buffer.saved"
            command = "file.reload"  # valid

            [[filetype]]
            tab_width = 2            # missing name

            [[lsp]]
            language = "rust"        # missing server
        "#,
        );

        // Valid entries still parse.
        assert_eq!(c.autocmds.len(), 1);
        assert_eq!(c.autocmds[0].event, "buffer.saved");
        assert!(c.filetypes.is_empty());
        assert!(c.lsps.is_empty());

        // Each dropped entry is reported, naming the missing field.
        let joined = c.parse_warnings.join("\n");
        assert!(
            c.parse_warnings.len() == 3,
            "expected 3 warnings, got {:?}",
            c.parse_warnings
        );
        assert!(joined.contains("[[autocmd]]") && joined.contains("`event`"));
        assert!(joined.contains("[[filetype]]") && joined.contains("`name`"));
        assert!(joined.contains("[[lsp]]") && joined.contains("`server`"));
    }

    #[test]
    fn load_surfaces_malformed_entry_warnings() {
        let dir =
            std::env::temp_dir().join(format!("ozone-config-badentry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[keymap]\n\"ctrl+s\" = \"file.save\"\n\n[[lsp]]\nlanguage = \"rust\"\n",
        )
        .unwrap();

        let (_c, warning) = Config::load_with_warning(&path);
        let warning = warning.expect("expected a malformed-entry warning");
        assert!(warning.contains("[[lsp]]") && warning.contains("`server`"));
        assert!(warning.contains("config.toml"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shipped_templates_parse_cleanly() {
        // The base template: editor/theme/ui only — list sections are split out.
        let base = Config::parse_str(Config::DEFAULT_TEMPLATE);
        assert_eq!(base.theme, "brewery-stout");
        assert!(base.lsps.is_empty());
        assert!(base.filetypes.is_empty());
        assert!(base.autocmds.is_empty());
        // Every shipped section template is valid TOML.
        for (name, tmpl) in Config::SECTION_TEMPLATES {
            assert!(
                tmpl.parse::<toml::Table>().is_ok(),
                "section template {name} does not parse"
            );
        }
    }

    #[test]
    fn section_files_merge_into_base_config() {
        let dir = std::env::temp_dir().join(format!("ozone_cfgdir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Base config defines editor + one keymap; section files add more.
        std::fs::write(
            dir.join("config.toml"),
            "[editor]\ntab_width = 2\n[keymap]\n\"ctrl+s\" = \"file.save\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("keymap.toml"),
            "[keymap]\n\"ctrl+z\" = \"edit.undo\"\n[keymap.rust]\n\"ctrl+b\" = \"build\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("lsp.toml"),
            "[[lsp]]\nlanguage = \"rust\"\nserver = \"rust-analyzer\"\n",
        )
        .unwrap();

        let (c, warning) = Config::load_with_warning(&dir.join("config.toml"));
        assert!(warning.is_none(), "unexpected: {warning:?}");
        // Base sections (editor) still come from config.toml.
        assert_eq!(c.editor.tab_width, 2);
        // keymap.toml exists, so it OWNS the keymap section — config.toml's
        // `ctrl+s` is ignored; only the file's two binds remain.
        assert_eq!(c.keymaps.len(), 2);
        assert!(!c.keymaps.iter().any(|k| k.keys == "ctrl+s"));
        assert!(c.keymaps.iter().any(|k| k.keys == "ctrl+z"));
        assert!(
            c.keymaps
                .iter()
                .any(|k| k.keys == "ctrl+b" && k.filetype.as_deref() == Some("rust"))
        );
        // lsp.toml owns the lsp section.
        assert_eq!(c.lsps.len(), 1);
        assert_eq!(c.lsps[0].server, "rust-analyzer");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_layout_is_split_and_loads() {
        let dir = std::env::temp_dir().join(format!("ozone_gen_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config.toml");

        Config::write_default_to(&config_path).unwrap();
        // The split layout is generated, not one big file.
        assert!(config_path.exists());
        for name in ["keymap.toml", "autocmd.toml", "filetype.toml", "lsp.toml"] {
            assert!(dir.join(name).exists(), "missing {name}");
        }
        // The generated config.toml has no list sections inline — parsing it
        // alone yields no lsps/filetypes (those live in their section files).
        let base = std::fs::read_to_string(&config_path).unwrap();
        let base_only = Config::parse_str(&base);
        assert!(base_only.lsps.is_empty() && base_only.filetypes.is_empty());

        // Loading the directory merges them back: rust LSP + markdown filetype +
        // the trim/format autocmds come from the section files.
        let (c, warning) = Config::load_with_warning(&config_path);
        assert!(warning.is_none(), "unexpected: {warning:?}");
        assert!(c.lsps.iter().any(|l| l.language == "rust"));
        assert!(c.filetypes.iter().any(|f| f.name == "markdown"));
        assert!(c.autocmds.iter().any(|a| a.command.contains("trim")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_config_without_keymap_warns_instead_of_rewriting() {
        // A config.toml written before the split layout existed: no
        // `[keymap]`/`[[keymap]]` and no sibling section files.
        let dir = std::env::temp_dir().join(format!("ozone_legacy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[editor]\nfont_size = 16\n").unwrap();

        // Bindings are config-driven with no hardcoded fallback, so a config
        // missing `[keymap]`/keymap.toml leaves every key (e.g. backspace)
        // unbound. Loading must surface that loudly rather than silently
        // backfilling files into the user's config directory.
        let (c, warning) = Config::load_with_warning(&path);
        assert!(c.keymaps.is_empty());
        let warning = warning.expect("expected a no-keybindings warning");
        assert!(warning.contains("no keybindings configured"));
        assert!(warning.contains("--reset-config"));

        // Nothing was written to disk.
        for name in ["keymap.toml", "autocmd.toml", "filetype.toml", "lsp.toml"] {
            assert!(!dir.join(name).exists(), "{name} should not be generated");
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[editor]\nfont_size = 16\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_at_overwrites_legacy_config_with_split_layout() {
        let dir = std::env::temp_dir().join(format!("ozone_reset_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // A pre-split config with no keymap, plus a stale section file that
        // should be overwritten (not just backfilled) by the reset.
        std::fs::write(&path, "[editor]\nfont_size = 99\n").unwrap();
        std::fs::write(dir.join("keymap.toml"), "[keymap]\n").unwrap();

        Config::reset_at(&path).unwrap();

        for name in ["keymap.toml", "autocmd.toml", "filetype.toml", "lsp.toml"] {
            assert!(dir.join(name).exists(), "missing {name}");
        }
        // The empty stale keymap.toml was overwritten with the shipped defaults.
        let (c, warning) = Config::load_with_warning(&path);
        assert!(warning.is_none(), "unexpected: {warning:?}");
        assert!(!c.keymaps.is_empty());
        assert!(c.keymaps.iter().any(|k| k.command == "file.save"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_keymap_table_form() {
        let c = Config::parse_str(
            r#"
            [keymap]
            "ctrl+s" = "file.save"
            "ctrl+z" = "edit.undo"

            [keymap.rust]
            "ctrl+shift+f" = "lsp.format"
        "#,
        );
        assert_eq!(c.keymaps.len(), 3);
        // Global binds carry no filetype.
        let save = c.keymaps.iter().find(|k| k.keys == "ctrl+s").unwrap();
        assert_eq!(save.command, "file.save");
        assert_eq!(save.filetype, None);
        // Nested table scopes binds to its filetype.
        let fmt = c.keymaps.iter().find(|k| k.keys == "ctrl+shift+f").unwrap();
        assert_eq!(fmt.command, "lsp.format");
        assert_eq!(fmt.filetype.as_deref(), Some("rust"));
    }

    #[test]
    fn lsp_capabilities_default_to_common_features() {
        // A `[[lsp]]` with no `[lsp.capabilities]` block gets sensible defaults.
        let c = Config::parse_str(
            r#"
            [[lsp]]
            language = "rust"
            server = "rust-analyzer"
        "#,
        );
        let caps = &c.lsps[0].capabilities;
        assert!(caps.diagnostics && caps.completion && caps.hover && caps.format);
        assert!(!caps.inlay_hints && !caps.semantic_tokens && !caps.code_lens);

        // A partial block overrides only the listed keys.
        let c = Config::parse_str(
            r#"
            [[lsp]]
            language = "rust"
            server = "rust-analyzer"
            [lsp.capabilities]
            inlay_hints = true
            format = false
        "#,
        );
        let caps = &c.lsps[0].capabilities;
        assert!(caps.inlay_hints, "explicitly enabled");
        assert!(!caps.format, "explicitly disabled");
        assert!(caps.diagnostics, "untouched key keeps its default");
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
