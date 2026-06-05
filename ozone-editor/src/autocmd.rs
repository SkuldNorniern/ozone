use ozone_config::AutocmdConfig;

use crate::events::{EditorEvent, EventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Autocommand {
    pub event: EventKind,
    pub pattern: String,
    pub command: String,
}

#[derive(Debug, Clone, Default)]
pub struct AutocommandRegistry {
    entries: Vec<Autocommand>,
}

impl AutocommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_config(configs: &[AutocmdConfig]) -> Self {
        let mut registry = Self::new();
        for config in configs {
            let Some(event) = EventKind::parse(&config.event) else {
                continue;
            };
            registry.register(event, &config.pattern, &config.command);
        }
        registry
    }

    pub fn register(&mut self, event: EventKind, pattern: &str, command: &str) {
        let pattern = normalized_pattern(pattern);
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        self.entries.push(Autocommand {
            event,
            pattern,
            command: command.to_string(),
        });
    }

    pub fn matching_commands(&self, event: &EditorEvent) -> Vec<&str> {
        let kind = event.kind();
        let match_text = event.match_text();
        self.entries
            .iter()
            .filter(|entry| entry.event == kind)
            .filter(|entry| pattern_matches(&entry.pattern, match_text.as_deref()))
            .map(|entry| entry.command.as_str())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn normalized_pattern(pattern: &str) -> String {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        "*".to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

fn pattern_matches(pattern: &str, text: Option<&str>) -> bool {
    if pattern == "*" {
        return true;
    }

    let Some(text) = text else {
        return false;
    };
    let text = text.to_ascii_lowercase();

    if let Some(ext) = pattern.strip_prefix("*.") {
        return text.ends_with(&format!(".{ext}"));
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return text.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return text.starts_with(prefix);
    }

    text == pattern
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ozone_buffer::BufferId;
    use ozone_config::AutocmdConfig;

    use super::*;

    #[test]
    fn builds_from_config_and_ignores_unknown_events() {
        let registry = AutocommandRegistry::from_config(&[
            AutocmdConfig {
                event: "buffer.pre-save".to_string(),
                pattern: "*".to_string(),
                command: "edit.trim-trailing-whitespace".to_string(),
            },
            AutocmdConfig {
                event: "not.real".to_string(),
                pattern: "*".to_string(),
                command: "ignored".to_string(),
            },
        ]);

        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn matches_filetype_events_by_exact_pattern() {
        let mut registry = AutocommandRegistry::new();
        registry.register(EventKind::BufferFiletype, "rust", "lsp.attach");
        registry.register(EventKind::BufferFiletype, "markdown", "editor.wrap");

        let event = EditorEvent::BufferFiletype {
            id: BufferId::next(),
            filetype: "rust".to_string(),
        };

        assert_eq!(registry.matching_commands(&event), vec!["lsp.attach"]);
    }

    #[test]
    fn matches_path_events_by_extension_or_wildcard() {
        let mut registry = AutocommandRegistry::new();
        registry.register(EventKind::BufferPreSave, "*.rs", "rust.save-hook");
        registry.register(EventKind::BufferPreSave, "*", "all.save-hook");

        let event = EditorEvent::BufferPreSave {
            id: BufferId::next(),
            path: PathBuf::from("src/main.rs"),
        };

        assert_eq!(
            registry.matching_commands(&event),
            vec!["rust.save-hook", "all.save-hook"]
        );
    }

    #[test]
    fn non_text_events_only_match_wildcard() {
        let mut registry = AutocommandRegistry::new();
        registry.register(EventKind::CursorMoved, "rust", "nope");
        registry.register(EventKind::CursorMoved, "*", "cursor.any");

        let event = EditorEvent::CursorMoved {
            view_id: crate::view::ViewId::next(),
            pos: ozone_buffer::Pos::new(2, 4),
        };

        assert_eq!(registry.matching_commands(&event), vec!["cursor.any"]);
    }
}
