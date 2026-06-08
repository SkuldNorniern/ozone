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

pub(super) fn parse_keymaps(table: &toml::Table) -> Vec<KeymapConfig> {
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
