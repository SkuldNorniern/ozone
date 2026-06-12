use super::{
    AutocmdConfig, FiletypeConfig, KeymapConfig, LineNumbers, LspCapabilities, LspConfig,
    ModifierOverrides,
};

pub(super) fn as_f32(value: Option<&toml::Value>) -> Option<f32> {
    match value {
        Some(toml::Value::Float(f)) => Some(*f as f32),
        Some(toml::Value::Integer(i)) => Some(*i as f32),
        _ => None,
    }
}

pub(super) fn as_usize(value: Option<&toml::Value>) -> Option<usize> {
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

/// Keymaps accept two forms:
///
/// 1. Array of tables (explicit, supports `filetype`):
///    ```toml
///    [[keymap]]
///    keys = "ctrl+s"
///    command = "file.save"
///    filetype = "rust"   # optional
///    ```
/// 2. Compact table — one line per binding (`chord = command`); a nested table
///    scopes its binds to a filetype:
///    ```toml
///    [keymap]
///    "ctrl+s" = "file.save"
///    [keymap.rust]
///    "ctrl+shift+f" = "lsp.format"
///    ```
/// A file uses one form (TOML forbids `keymap` being both a table and an
/// array-of-tables); both are accepted here.
pub(super) fn parse_keymaps(table: &toml::Table) -> Vec<KeymapConfig> {
    match table.get("keymap") {
        Some(toml::Value::Array(_)) => parse_keymap_array(table),
        Some(toml::Value::Table(t)) => parse_keymap_table(t),
        _ => Vec::new(),
    }
}

fn parse_keymap_array(table: &toml::Table) -> Vec<KeymapConfig> {
    table_array(table, "keymap")
        .filter_map(|entry| {
            Some(KeymapConfig {
                keys: non_empty_string(entry, "keys")?,
                command: non_empty_string(entry, "command")?,
                filetype: non_empty_string(entry, "filetype"),
                platform: non_empty_string(entry, "platform"),
            })
        })
        .collect()
}

fn parse_keymap_table(t: &toml::Table) -> Vec<KeymapConfig> {
    let mut out = Vec::new();
    for (key, value) in t {
        match value {
            // `"chord" = "command"` — a global binding.
            toml::Value::String(command) => {
                push_bind(&mut out, key, command, None);
            }
            // `[keymap.<filetype>]` — filetype-scoped binds.
            toml::Value::Table(sub) => {
                for (chord, cmd) in sub {
                    if let toml::Value::String(command) = cmd {
                        push_bind(&mut out, chord, command, Some(key.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn push_bind(out: &mut Vec<KeymapConfig>, chord: &str, command: &str, filetype: Option<String>) {
    let chord = chord.trim();
    let command = command.trim();
    if !chord.is_empty() && !command.is_empty() {
        out.push(KeymapConfig {
            keys: chord.to_string(),
            command: command.to_string(),
            filetype,
            platform: None,
        });
    }
}

pub(super) fn parse_autocmds(table: &toml::Table) -> Vec<AutocmdConfig> {
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

pub(super) fn parse_filetypes(table: &toml::Table) -> Vec<FiletypeConfig> {
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

pub(super) fn parse_lsps(table: &toml::Table) -> Vec<LspConfig> {
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
    // Start from the sensible defaults; only present keys override them, so a
    // `[lsp.capabilities]` block can list just what it wants to change.
    let mut caps = LspCapabilities::default();
    let Some(t) = entry.get("capabilities").and_then(|v| v.as_table()) else {
        return caps;
    };
    let or = |key: &str, current: bool| t.get(key).and_then(|v| v.as_bool()).unwrap_or(current);
    caps.completion = or("completion", caps.completion);
    caps.diagnostics = or("diagnostics", caps.diagnostics);
    caps.hover = or("hover", caps.hover);
    caps.goto_definition = or("goto_definition", caps.goto_definition);
    caps.find_references = or("find_references", caps.find_references);
    caps.rename = or("rename", caps.rename);
    caps.format = or("format", caps.format);
    caps.code_actions = or("code_actions", caps.code_actions);
    caps.inlay_hints = or("inlay_hints", caps.inlay_hints);
    caps.semantic_tokens = or("semantic_tokens", caps.semantic_tokens);
    caps.code_lens = or("code_lens", caps.code_lens);
    caps
}

pub(super) fn parse_modifiers(table: &toml::Table) -> ModifierOverrides {
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
