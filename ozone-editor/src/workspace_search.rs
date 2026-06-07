//! Bounded literal search across workspace text files.

use std::path::{Path, PathBuf};

use crate::commands::collect_workspace_files;
use crate::search::find_matches;

pub const MAX_SEARCH_FILES: usize = 20_000;
pub const MAX_SEARCH_RESULTS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

impl WorkspaceMatch {
    /// Render a one-based reference row for a `References` buffer.
    pub fn display(&self) -> String {
        format!(
            "{}:{}:{}: {}",
            self.path.to_string_lossy().replace('\\', "/"),
            self.line + 1,
            self.column + 1,
            self.preview
        )
    }

    /// Parse a row produced by [`display`](Self::display).
    pub fn parse(row: &str) -> Option<Self> {
        let mut parts = row.splitn(4, ':');
        let path = PathBuf::from(parts.next()?.trim());
        let line = parts.next()?.parse::<usize>().ok()?.checked_sub(1)?;
        let column = parts.next()?.parse::<usize>().ok()?.checked_sub(1)?;
        let preview = parts.next()?.trim_start().to_string();
        Some(Self {
            path,
            line,
            column,
            preview,
        })
    }
}

/// Search UTF-8 files below `base`, returning at most `result_cap` matches.
pub fn search_workspace(
    base: &Path,
    query: &str,
    file_cap: usize,
    result_cap: usize,
) -> Vec<WorkspaceMatch> {
    if query.is_empty() || result_cap == 0 {
        return Vec::new();
    }

    let mut results = Vec::new();
    for relative in collect_workspace_files(base, file_cap) {
        let Ok(text) = std::fs::read_to_string(base.join(&relative)) else {
            continue;
        };
        for (line, content) in text.lines().enumerate() {
            for column in find_matches(content, query, false) {
                results.push(WorkspaceMatch {
                    path: PathBuf::from(&relative),
                    line,
                    column,
                    preview: content.trim().to_string(),
                });
                if results.len() >= result_cap {
                    return results;
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_rows_round_trip() {
        let hit = WorkspaceMatch {
            path: PathBuf::from("src/main.rs"),
            line: 11,
            column: 4,
            preview: "let value = needle;".to_string(),
        };
        assert_eq!(WorkspaceMatch::parse(&hit.display()), Some(hit));
    }

    #[test]
    fn searches_text_files_and_skips_ignored_directories() {
        let base =
            std::env::temp_dir().join(format!("ozone_workspace_search_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::create_dir_all(base.join("target")).unwrap();
        std::fs::write(
            base.join("src").join("one.rs"),
            "Needle here\nand needle again",
        )
        .unwrap();
        std::fs::write(base.join("target").join("ignored.rs"), "needle").unwrap();

        let hits = search_workspace(&base, "needle", 100, 100);
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|hit| hit.path == PathBuf::from("src/one.rs"))
        );
        assert_eq!((hits[0].line, hits[0].column), (0, 0));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn respects_result_cap() {
        let base =
            std::env::temp_dir().join(format!("ozone_workspace_search_cap_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("many.txt"), "x x x x x").unwrap();

        assert_eq!(search_workspace(&base, "x", 100, 3).len(), 3);

        let _ = std::fs::remove_dir_all(&base);
    }
}
