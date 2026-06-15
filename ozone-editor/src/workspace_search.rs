//! Bounded literal search across workspace text files.

use std::fs;
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
/// Skips binary files (any null byte in the first 8 KiB).
pub fn search_workspace(
    base: &Path,
    query: &str,
    file_cap: usize,
    result_cap: usize,
) -> Vec<WorkspaceMatch> {
    search_workspace_inner(base, query, file_cap, result_cap, None)
}

/// Like [`search_workspace`] but calls `on_progress(files_scanned)` after
/// each file so callers can report progress without re-scanning.
pub fn search_workspace_with_progress(
    base: &Path,
    query: &str,
    file_cap: usize,
    result_cap: usize,
    on_progress: impl Fn(usize),
) -> Vec<WorkspaceMatch> {
    search_workspace_inner(base, query, file_cap, result_cap, Some(&on_progress))
}

fn search_workspace_inner(
    base: &Path,
    query: &str,
    file_cap: usize,
    result_cap: usize,
    on_progress: Option<&dyn Fn(usize)>,
) -> Vec<WorkspaceMatch> {
    if query.is_empty() || result_cap == 0 {
        return Vec::new();
    }

    let mut results = Vec::new();
    for (idx, relative) in collect_workspace_files(base, file_cap)
        .into_iter()
        .enumerate()
    {
        let Ok(bytes) = fs::read(base.join(&relative)) else {
            continue;
        };
        if looks_like_binary(&bytes) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
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
        if let Some(cb) = on_progress {
            cb(idx + 1);
        }
    }
    results
}

/// Returns `true` if `bytes` looks like a binary file (null byte in first 8 KiB).
fn looks_like_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::process;

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
        let base = env::temp_dir().join(format!("ozone_workspace_search_{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src")).unwrap();
        fs::create_dir_all(base.join("target")).unwrap();
        fs::write(
            base.join("src").join("one.rs"),
            "Needle here\nand needle again",
        )
        .unwrap();
        fs::write(base.join("target").join("ignored.rs"), "needle").unwrap();

        let hits = search_workspace(&base, "needle", 100, 100);
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|hit| hit.path.as_path() == Path::new("src/one.rs"))
        );
        assert_eq!((hits[0].line, hits[0].column), (0, 0));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn respects_result_cap() {
        let base = env::temp_dir().join(format!("ozone_workspace_search_cap_{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("many.txt"), "x x x x x").unwrap();

        assert_eq!(search_workspace(&base, "x", 100, 3).len(), 3);

        let _ = fs::remove_dir_all(&base);
    }
}
