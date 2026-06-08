//! Runtime theme loading and shared paint helpers.
//!
//! `config.toml` selects a theme by name or path. The palette lives in a
//! separate TOML file; missing files and invalid fields retain safe defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock, RwLockReadGuard};

use aurea::render::{Color, Paint, PaintStyle};
use ozone_syntax::TokenKind;

static THEME: OnceLock<RwLock<Theme>> = OnceLock::new();
const BUNDLED_THEMES: [(&str, &str); 3] = [
    (
        "brewery-stout",
        include_str!("../../themes/brewery-stout.toml"),
    ),
    (
        "brewery-wine",
        include_str!("../../themes/brewery-wine.toml"),
    ),
    (
        "catppuccin-mocha",
        include_str!("../../themes/catppuccin-mocha.toml"),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SyntaxTheme {
    pub default: Color,
    pub keyword: Color,
    pub keyword_control: Color,
    pub type_: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    pub macro_: Color,
    pub attribute: Color,
    pub lifetime: Color,
    pub function: Color,
    pub operator: Color,
}

#[derive(Debug, Clone)]
pub(crate) struct Theme {
    pub background: Color,
    pub gutter: Color,
    pub foreground: Color,
    pub line_number: Color,
    pub line_number_active: Color,
    pub border: Color,
    pub cursor: Color,
    pub cursor_line: Color,
    pub selection: Color,
    pub active_pane_border: Color,
    pub bracket_match: Color,
    pub scrollbar_thumb: Color,
    pub statusbar_bg: Color,
    pub statusbar_fg: Color,
    pub statusbar_dim: Color,
    pub status_mode_bg: Color,
    pub scrim: Color,
    pub picker_bg: Color,
    pub picker_border: Color,
    pub picker_input_bg: Color,
    pub picker_selection: Color,
    pub picker_fg: Color,
    pub picker_detail: Color,
    pub picker_prompt: Color,
    pub search_match: Color,
    pub search_match_active: Color,
    pub notify_info: Color,
    pub notify_success: Color,
    pub notify_warn: Color,
    pub notify_error: Color,
    pub syntax: SyntaxTheme,
    pub terminal_ansi: [Color; 16],
}

impl Default for Theme {
    fn default() -> Self {
        let red = Color::rgb(243, 139, 168);
        let green = Color::rgb(166, 227, 161);
        let yellow = Color::rgb(249, 226, 175);
        let blue = Color::rgb(137, 180, 250);
        let pink = Color::rgb(245, 194, 231);
        let cyan = Color::rgb(148, 226, 213);
        Self {
            background: Color::rgb(30, 30, 46),
            gutter: Color::rgb(24, 24, 37),
            foreground: Color::rgb(205, 214, 244),
            line_number: Color::rgb(88, 91, 112),
            line_number_active: Color::rgb(205, 214, 244),
            border: Color::rgb(49, 50, 68),
            cursor: Color::rgba(245, 224, 220, 220),
            cursor_line: Color::rgba(49, 50, 68, 140),
            selection: Color::rgba(69, 71, 90, 210),
            active_pane_border: blue,
            bracket_match: Color::rgba(137, 180, 250, 70),
            scrollbar_thumb: Color::rgba(88, 91, 112, 180),
            statusbar_bg: Color::rgb(24, 24, 37),
            statusbar_fg: green,
            statusbar_dim: blue,
            status_mode_bg: Color::rgb(49, 50, 68),
            scrim: Color::rgba(0, 0, 0, 110),
            picker_bg: Color::rgb(24, 24, 37),
            picker_border: Color::rgb(69, 71, 90),
            picker_input_bg: Color::rgb(17, 17, 27),
            picker_selection: Color::rgb(49, 50, 68),
            picker_fg: Color::rgb(205, 214, 244),
            picker_detail: Color::rgb(127, 132, 156),
            picker_prompt: Color::rgb(203, 166, 247),
            search_match: Color::rgba(249, 226, 175, 70),
            search_match_active: Color::rgba(250, 179, 135, 150),
            notify_info: blue,
            notify_success: green,
            notify_warn: yellow,
            notify_error: red,
            syntax: SyntaxTheme {
                default: Color::rgb(205, 214, 244),
                keyword: Color::rgb(203, 166, 247),
                keyword_control: red,
                type_: blue,
                string: green,
                comment: Color::rgb(88, 91, 112),
                number: Color::rgb(250, 179, 135),
                macro_: Color::rgb(137, 220, 235),
                attribute: pink,
                lifetime: pink,
                function: blue,
                operator: Color::rgb(137, 220, 235),
            },
            terminal_ansi: [
                Color::rgb(69, 71, 90),
                red,
                green,
                yellow,
                blue,
                pink,
                cyan,
                Color::rgb(186, 194, 222),
                Color::rgb(88, 91, 112),
                red,
                green,
                yellow,
                blue,
                pink,
                cyan,
                Color::rgb(166, 173, 200),
            ],
        }
    }
}

impl Theme {
    fn load(selection: &str) -> Self {
        let mut theme = Self::default();
        let text = resolve_theme_text(selection);
        let Some(text) = text.as_deref() else {
            return theme;
        };
        let Ok(table) = text.parse::<toml::Table>() else {
            return theme;
        };
        theme.apply_table(&table);
        theme
    }

    fn apply_table(&mut self, table: &toml::Table) {
        let ui = table.get("ui").and_then(toml::Value::as_table);
        apply(ui, "background", &mut self.background);
        apply(ui, "gutter", &mut self.gutter);
        apply(ui, "foreground", &mut self.foreground);
        apply(ui, "line_number", &mut self.line_number);
        apply(ui, "line_number_active", &mut self.line_number_active);
        apply(ui, "border", &mut self.border);
        apply(ui, "cursor", &mut self.cursor);
        apply(ui, "cursor_line", &mut self.cursor_line);
        apply(ui, "selection", &mut self.selection);
        apply(ui, "active_pane_border", &mut self.active_pane_border);
        apply(ui, "bracket_match", &mut self.bracket_match);
        apply(ui, "scrollbar_thumb", &mut self.scrollbar_thumb);
        apply(ui, "statusbar_bg", &mut self.statusbar_bg);
        apply(ui, "statusbar_fg", &mut self.statusbar_fg);
        apply(ui, "statusbar_dim", &mut self.statusbar_dim);
        apply(ui, "status_mode_bg", &mut self.status_mode_bg);
        apply(ui, "scrim", &mut self.scrim);
        apply(ui, "picker_bg", &mut self.picker_bg);
        apply(ui, "picker_border", &mut self.picker_border);
        apply(ui, "picker_input_bg", &mut self.picker_input_bg);
        apply(ui, "picker_selection", &mut self.picker_selection);
        apply(ui, "picker_fg", &mut self.picker_fg);
        apply(ui, "picker_detail", &mut self.picker_detail);
        apply(ui, "picker_prompt", &mut self.picker_prompt);
        apply(ui, "search_match", &mut self.search_match);
        apply(ui, "search_match_active", &mut self.search_match_active);

        let syntax = table.get("syntax").and_then(toml::Value::as_table);
        apply(syntax, "default", &mut self.syntax.default);
        apply(syntax, "keyword", &mut self.syntax.keyword);
        apply(syntax, "keyword_control", &mut self.syntax.keyword_control);
        apply(syntax, "type", &mut self.syntax.type_);
        apply(syntax, "string", &mut self.syntax.string);
        apply(syntax, "comment", &mut self.syntax.comment);
        apply(syntax, "number", &mut self.syntax.number);
        apply(syntax, "macro", &mut self.syntax.macro_);
        apply(syntax, "attribute", &mut self.syntax.attribute);
        apply(syntax, "lifetime", &mut self.syntax.lifetime);
        apply(syntax, "function", &mut self.syntax.function);
        apply(syntax, "operator", &mut self.syntax.operator);

        let lsp = table.get("lsp").and_then(toml::Value::as_table);
        apply(lsp, "info", &mut self.notify_info);
        apply(lsp, "hint", &mut self.notify_success);
        apply(lsp, "warning", &mut self.notify_warn);
        apply(lsp, "error", &mut self.notify_error);

        if let Some(ansi) = table
            .get("terminal")
            .and_then(toml::Value::as_table)
            .and_then(|terminal| terminal.get("ansi"))
            .and_then(toml::Value::as_array)
            && ansi.len() == 16
        {
            for (target, value) in self.terminal_ansi.iter_mut().zip(ansi) {
                if let Some(color) = value.as_str().and_then(parse_hex_color) {
                    *target = color;
                }
            }
        }
    }
}

pub(crate) fn initialize(selection: &str) {
    ensure_bundled_themes();
    let store = THEME.get_or_init(|| RwLock::new(Theme::default()));
    *store
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Theme::load(selection);
}

/// Write the bundled themes into the user config `themes/` directory (creating
/// it) when missing, so users have editable templates and a place to drop their
/// own. Existing files are never overwritten. Best-effort; failures are ignored.
pub(crate) fn ensure_bundled_themes() {
    let Some(config_path) = ozone_config::Config::user_config_path() else {
        return;
    };
    let Some(config_dir) = config_path.parent() else {
        return;
    };
    let dir = config_dir.join("themes");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (id, text) in BUNDLED_THEMES {
        let path = dir.join(format!("{id}.toml"));
        if !path.exists() {
            let _ = std::fs::write(&path, text);
        }
    }
}

/// Persist the chosen theme id into the user config's `[theme].name`, editing in
/// place so comments and the rest of the file are preserved. Best-effort.
pub(crate) fn persist_theme_name(id: &str) {
    let Some(path) = ozone_config::Config::user_config_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::write(&path, update_theme_name(&text, id));
}

/// Return `text` with `[theme].name` set to `id`, preserving everything else.
/// Replaces an existing `name` in `[theme]`, inserts one if the block lacks it,
/// or appends a `[theme]` block if absent.
fn update_theme_name(text: &str, id: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: Vec<String> = Vec::new();
    let mut in_theme = false;
    let mut replaced = false;
    let mut theme_header_idx: Option<usize> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // Leaving the previous section; entering a new one.
            in_theme = trimmed == "[theme]";
            if in_theme {
                theme_header_idx = Some(out.len());
            }
        } else if in_theme && !replaced {
            let key = trimmed.split('=').next().unwrap_or("").trim();
            if key == "name" {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                out.push(format!("{indent}name = \"{id}\""));
                replaced = true;
                continue;
            }
        }
        out.push(line.to_string());
    }

    if !replaced {
        match theme_header_idx {
            // `[theme]` exists but had no `name` → insert right after the header.
            Some(idx) => out.insert(idx + 1, format!("name = \"{id}\"")),
            // No `[theme]` block at all → append one.
            None => {
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push("[theme]".to_string());
                out.push(format!("name = \"{id}\""));
            }
        }
    }

    let mut result = out.join(newline);
    if text.ends_with('\n') {
        result.push_str(newline);
    }
    result
}

pub(crate) fn activate(selection: &str) -> bool {
    let Some(text) = resolve_theme_text(selection) else {
        return false;
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return false;
    };
    let mut theme = Theme::default();
    theme.apply_table(&table);
    let store = THEME.get_or_init(|| RwLock::new(Theme::default()));
    *store
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = theme;
    true
}

pub(crate) fn palette() -> RwLockReadGuard<'static, Theme> {
    THEME
        .get_or_init(|| RwLock::new(Theme::default()))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn available_themes() -> Vec<ThemeInfo> {
    let mut themes = BTreeMap::<String, ThemeInfo>::new();
    for (id, text) in BUNDLED_THEMES {
        let name = theme_name(text).unwrap_or_else(|| id.to_string());
        themes.insert(
            id.to_string(),
            ThemeInfo {
                id: id.to_string(),
                name,
            },
        );
    }
    for directory in theme_directories() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| theme_name(&text))
                .unwrap_or_else(|| id.clone());
            themes.insert(id.clone(), ThemeInfo { id, name });
        }
    }
    themes.into_values().collect()
}

fn resolve_theme_text(selection: &str) -> Option<String> {
    if let Some(path) = resolve_theme_path(selection)
        && let Ok(text) = std::fs::read_to_string(path)
    {
        return Some(text);
    }
    let id = Path::new(selection).file_stem()?.to_str()?;
    BUNDLED_THEMES
        .iter()
        .find(|(bundled_id, _)| *bundled_id == id)
        .map(|(_, text)| (*text).to_string())
}

fn theme_name(text: &str) -> Option<String> {
    text.parse::<toml::Table>()
        .ok()?
        .get("meta")?
        .as_table()?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

fn resolve_theme_path(selection: &str) -> Option<PathBuf> {
    let requested = Path::new(selection);
    if requested.is_file() {
        return Some(requested.to_path_buf());
    }
    let file_name = if requested.extension().is_some() {
        requested.to_path_buf()
    } else {
        PathBuf::from(format!("{selection}.toml"))
    };
    for directory in theme_directories() {
        let path = directory.join(&file_name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn theme_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(config_path) = ozone_config::Config::user_config_path()
        && let Some(config_dir) = config_path.parent()
    {
        directories.push(config_dir.join("themes"));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_dir) = executable.parent()
    {
        directories.push(executable_dir.join("themes"));
    }
    directories.push(PathBuf::from("themes"));
    directories
}

fn apply(table: Option<&toml::Table>, key: &str, target: &mut Color) {
    if let Some(color) = table
        .and_then(|t| t.get(key))
        .and_then(toml::Value::as_str)
        .and_then(parse_hex_color)
    {
        *target = color;
    }
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#')?;
    let byte = |start| u8::from_str_radix(&hex[start..start + 2], 16).ok();
    match hex.len() {
        6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

pub(crate) fn notify_accent(level: ozone_editor::NotifyLevel) -> Color {
    let p = palette();
    match level {
        ozone_editor::NotifyLevel::Info => p.notify_info,
        ozone_editor::NotifyLevel::Success => p.notify_success,
        ozone_editor::NotifyLevel::Warn => p.notify_warn,
        ozone_editor::NotifyLevel::Error => p.notify_error,
    }
}

pub(crate) fn token_color(kind: TokenKind) -> Color {
    let s = palette().syntax;
    match kind {
        TokenKind::Keyword => s.keyword,
        TokenKind::KeywordControl => s.keyword_control,
        TokenKind::Type => s.type_,
        TokenKind::String => s.string,
        TokenKind::Comment => s.comment,
        TokenKind::Number => s.number,
        TokenKind::Macro => s.macro_,
        TokenKind::Attribute => s.attribute,
        TokenKind::Lifetime => s.lifetime,
        TokenKind::Function => s.function,
        TokenKind::Operator => s.operator,
        TokenKind::SectionHeader => s.keyword,
        _ => s.default,
    }
}

fn xterm256(idx: u8) -> Color {
    match idx {
        0..=15 => palette().terminal_ansi[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::rgb(scale(i / 36), scale((i % 36) / 6), scale(i % 6))
        }
        _ => {
            let v = 8 + (idx - 232) * 10;
            Color::rgb(v, v, v)
        }
    }
}

pub(crate) fn term_color(c: ozone_term::Color, default: Color) -> Color {
    match c {
        ozone_term::Color::Default => default,
        ozone_term::Color::Indexed(i) => xterm256(i),
        ozone_term::Color::Rgb(r, g, b) => Color::rgb(r, g, b),
    }
}

pub(crate) fn solid(c: Color) -> Paint {
    Paint::new().color(c).style(PaintStyle::Fill)
}

pub(crate) fn stroke(c: Color, w: f32) -> Paint {
    Paint::new()
        .color(c)
        .style(PaintStyle::Stroke)
        .stroke_width(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_and_rgba_hex() {
        assert!(parse_hex_color("#112233").is_some());
        assert!(parse_hex_color("#11223344").is_some());
        assert!(parse_hex_color("112233").is_none());
        assert!(parse_hex_color("#xyzxyz").is_none());
    }

    #[test]
    fn invalid_fields_keep_defaults() {
        let mut theme = Theme::default();
        let before = theme.background;
        let table = "[ui]\nbackground = \"invalid\"\n"
            .parse::<toml::Table>()
            .unwrap();
        theme.apply_table(&table);
        assert_eq!(theme.background, before);
    }

    #[test]
    fn bundled_brewery_themes_are_discoverable() {
        let ids: Vec<String> = available_themes()
            .into_iter()
            .map(|theme| theme.id)
            .collect();
        assert!(ids.iter().any(|id| id == "brewery-stout"));
        assert!(ids.iter().any(|id| id == "brewery-wine"));
        assert!(resolve_theme_text("brewery-stout").is_some());
    }

    #[test]
    fn update_theme_name_replaces_existing() {
        let cfg = "[editor]\nfont_size = 14\n\n[theme]\nname = \"brewery-stout\"\n";
        let out = update_theme_name(cfg, "brewery-wine");
        assert!(out.contains("name = \"brewery-wine\""));
        assert!(!out.contains("brewery-stout"));
        assert!(out.contains("[editor]")); // other sections preserved
    }

    #[test]
    fn update_theme_name_inserts_when_block_lacks_name() {
        let cfg = "[theme]\n# pick one\n";
        let out = update_theme_name(cfg, "catppuccin-mocha");
        assert!(out.contains("[theme]"));
        assert!(out.contains("name = \"catppuccin-mocha\""));
        assert!(out.contains("# pick one")); // comment kept
    }

    #[test]
    fn update_theme_name_appends_block_when_absent() {
        let cfg = "[editor]\nfont_size = 14\n";
        let out = update_theme_name(cfg, "brewery-wine");
        assert!(out.contains("[theme]"));
        assert!(out.contains("name = \"brewery-wine\""));
        assert!(out.contains("[editor]"));
    }

    #[test]
    fn terminal_palette_accepts_sixteen_colors() {
        let mut theme = Theme::default();
        let table = include_str!("../../themes/brewery-stout.toml")
            .parse::<toml::Table>()
            .unwrap();
        theme.apply_table(&table);
        assert_eq!(theme.terminal_ansi[0], parse_hex_color("#493c57").unwrap());
        assert_eq!(theme.terminal_ansi[15], parse_hex_color("#e4e3df").unwrap());
    }
}
