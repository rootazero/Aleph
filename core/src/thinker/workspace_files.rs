//! Standardized workspace files for system prompt injection.
//!
//! Loads user-editable workspace files (SOUL.md, IDENTITY.md, etc.) from the
//! workspace directory, applying per-file and total budget constraints.

use std::path::{Path, PathBuf};

use crate::thinker::prompt_budget::truncate_with_head_tail;

/// Canonical workspace file names, loaded in this order.
pub const WORKSPACE_FILE_NAMES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
];

/// Configuration for workspace file loading and truncation.
#[derive(Debug, Clone)]
pub struct WorkspaceFilesConfig {
    /// Maximum characters per individual file before truncation.
    pub per_file_max_chars: usize,
    /// Maximum total characters across all loaded files.
    pub total_max_chars: usize,
}

impl Default for WorkspaceFilesConfig {
    fn default() -> Self {
        Self {
            per_file_max_chars: 20_000,
            total_max_chars: 100_000,
        }
    }
}

/// A single loaded workspace file with truncation metadata.
#[derive(Debug, Clone)]
pub struct WorkspaceFile {
    /// Canonical file name (e.g. "SOUL.md").
    pub name: &'static str,
    /// File content after truncation, or None if not found / empty.
    pub content: Option<String>,
    /// Whether the content was truncated to fit budget.
    pub truncated: bool,
    /// Original byte size before truncation (0 if not found).
    pub original_size: usize,
}

/// Collection of loaded workspace files from a workspace directory.
#[derive(Debug, Clone)]
pub struct WorkspaceFiles {
    /// The workspace directory these files were loaded from.
    pub workspace_dir: PathBuf,
    /// Loaded files in canonical order.
    pub files: Vec<WorkspaceFile>,
}

/// Resolve the path for a workspace file.
///
/// Checks `.aleph/<filename>` first, then `<workspace>/<filename>`.
/// Returns the first path that exists, or None.
pub fn resolve_path(workspace: &Path, filename: &str) -> Option<PathBuf> {
    let aleph_path = workspace.join(".aleph").join(filename);
    if aleph_path.is_file() {
        return Some(aleph_path);
    }
    let root_path = workspace.join(filename);
    if root_path.is_file() {
        return Some(root_path);
    }
    None
}

impl WorkspaceFiles {
    /// Load all workspace files from the given directory, applying truncation.
    ///
    /// Files are loaded in `WORKSPACE_FILE_NAMES` order. Each file is
    /// individually capped at `config.per_file_max_chars`, and the total
    /// across all files is capped at `config.total_max_chars`.
    pub fn load(workspace: &Path, config: &WorkspaceFilesConfig) -> Self {
        let mut files = Vec::with_capacity(WORKSPACE_FILE_NAMES.len());
        let mut total_chars = 0usize;

        for &name in WORKSPACE_FILE_NAMES {
            let path = resolve_path(workspace, name);

            let raw = path.and_then(|p| std::fs::read_to_string(p).ok());

            // Skip missing or empty files
            let raw = match raw {
                Some(ref s) if !s.trim().is_empty() => s,
                _ => {
                    files.push(WorkspaceFile {
                        name,
                        content: None,
                        truncated: false,
                        original_size: 0,
                    });
                    continue;
                }
            };

            let original_size = raw.len();

            // Apply per-file truncation
            let remaining_budget = config.total_max_chars.saturating_sub(total_chars);
            let effective_limit = config.per_file_max_chars.min(remaining_budget);

            if effective_limit == 0 {
                // Total budget exhausted
                files.push(WorkspaceFile {
                    name,
                    content: None,
                    truncated: true,
                    original_size,
                });
                continue;
            }

            let (content, truncated) = if raw.len() > effective_limit {
                let truncated_content =
                    truncate_with_head_tail(raw, effective_limit, 0.7, 0.2);
                (truncated_content, true)
            } else {
                (raw.clone(), false)
            };

            total_chars += content.len();
            files.push(WorkspaceFile {
                name,
                content: Some(content),
                truncated,
                original_size,
            });
        }

        Self {
            workspace_dir: workspace.to_path_buf(),
            files,
        }
    }

    /// Get the content of a workspace file by name.
    ///
    /// Returns the (possibly truncated) content, or None if not loaded.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.content.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn workspace_file_names_match_spec() {
        assert_eq!(WORKSPACE_FILE_NAMES.len(), 7);
        assert_eq!(WORKSPACE_FILE_NAMES[0], "SOUL.md");
        assert_eq!(WORKSPACE_FILE_NAMES[1], "IDENTITY.md");
        assert_eq!(WORKSPACE_FILE_NAMES[2], "AGENTS.md");
        assert_eq!(WORKSPACE_FILE_NAMES[3], "TOOLS.md");
        assert_eq!(WORKSPACE_FILE_NAMES[4], "MEMORY.md");
        assert_eq!(WORKSPACE_FILE_NAMES[5], "HEARTBEAT.md");
        assert_eq!(WORKSPACE_FILE_NAMES[6], "BOOTSTRAP.md");
    }

    #[test]
    fn default_config_values() {
        let config = WorkspaceFilesConfig::default();
        assert_eq!(config.per_file_max_chars, 20_000);
        assert_eq!(config.total_max_chars, 100_000);
    }

    #[test]
    fn load_finds_existing_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "You are Aleph.").unwrap();
        fs::write(dir.path().join("IDENTITY.md"), "Name: Aleph").unwrap();

        let config = WorkspaceFilesConfig::default();
        let ws = WorkspaceFiles::load(dir.path(), &config);

        assert_eq!(ws.files.len(), WORKSPACE_FILE_NAMES.len());
        assert_eq!(ws.get("SOUL.md"), Some("You are Aleph."));
        assert_eq!(ws.get("IDENTITY.md"), Some("Name: Aleph"));
    }

    #[test]
    fn load_skips_missing_files() {
        let dir = TempDir::new().unwrap();
        // No files created

        let config = WorkspaceFilesConfig::default();
        let ws = WorkspaceFiles::load(dir.path(), &config);

        for file in &ws.files {
            assert!(file.content.is_none());
            assert!(!file.truncated);
            assert_eq!(file.original_size, 0);
        }
    }

    #[test]
    fn load_skips_empty_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "").unwrap();
        fs::write(dir.path().join("IDENTITY.md"), "   \n  ").unwrap();

        let config = WorkspaceFilesConfig::default();
        let ws = WorkspaceFiles::load(dir.path(), &config);

        assert!(ws.get("SOUL.md").is_none());
        assert!(ws.get("IDENTITY.md").is_none());
    }

    #[test]
    fn load_truncates_large_files() {
        let dir = TempDir::new().unwrap();
        let large_content = "A".repeat(5000);
        fs::write(dir.path().join("SOUL.md"), &large_content).unwrap();

        let config = WorkspaceFilesConfig {
            per_file_max_chars: 200,
            total_max_chars: 100_000,
        };
        let ws = WorkspaceFiles::load(dir.path(), &config);

        let soul = ws.files.iter().find(|f| f.name == "SOUL.md").unwrap();
        assert!(soul.truncated);
        assert_eq!(soul.original_size, 5000);
        let content = soul.content.as_ref().unwrap();
        assert!(content.len() < 5000);
        assert!(content.contains("[..."));
        assert!(content.contains("truncated ...]"));
    }

    #[test]
    fn load_respects_total_budget() {
        let dir = TempDir::new().unwrap();
        // Each file 500 chars, total budget 900 — not all can fit
        for name in WORKSPACE_FILE_NAMES {
            fs::write(dir.path().join(name), "X".repeat(500)).unwrap();
        }

        let config = WorkspaceFilesConfig {
            per_file_max_chars: 10_000,
            total_max_chars: 900,
        };
        let ws = WorkspaceFiles::load(dir.path(), &config);

        let total: usize = ws
            .files
            .iter()
            .filter_map(|f| f.content.as_ref().map(|c| c.len()))
            .sum();
        assert!(total <= 900, "Total {} exceeded budget 900", total);

        // First file should be loaded fully (500 < per_file and < total)
        assert!(ws.get("SOUL.md").is_some());

        // Some later files should be truncated or skipped
        let skipped_or_truncated = ws
            .files
            .iter()
            .filter(|f| f.original_size > 0 && (f.content.is_none() || f.truncated))
            .count();
        assert!(skipped_or_truncated > 0, "Budget should cause truncation");
    }

    #[test]
    fn get_returns_none_for_unknown_name() {
        let dir = TempDir::new().unwrap();
        let config = WorkspaceFilesConfig::default();
        let ws = WorkspaceFiles::load(dir.path(), &config);

        assert!(ws.get("NONEXISTENT.md").is_none());
    }

    #[test]
    fn get_returns_content_by_name() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("TOOLS.md"), "tool: bash").unwrap();

        let config = WorkspaceFilesConfig::default();
        let ws = WorkspaceFiles::load(dir.path(), &config);

        assert_eq!(ws.get("TOOLS.md"), Some("tool: bash"));
        assert!(ws.get("SOUL.md").is_none());
    }

    #[test]
    fn resolve_path_prefers_aleph_directory() {
        let dir = TempDir::new().unwrap();
        let aleph_dir = dir.path().join(".aleph");
        fs::create_dir_all(&aleph_dir).unwrap();

        // File exists in both root and .aleph/
        fs::write(dir.path().join("SOUL.md"), "root version").unwrap();
        fs::write(aleph_dir.join("SOUL.md"), "aleph version").unwrap();

        let resolved = resolve_path(dir.path(), "SOUL.md").unwrap();
        assert_eq!(resolved, aleph_dir.join("SOUL.md"));

        // Load should pick the .aleph/ version
        let config = WorkspaceFilesConfig::default();
        let ws = WorkspaceFiles::load(dir.path(), &config);
        assert_eq!(ws.get("SOUL.md"), Some("aleph version"));
    }

    #[test]
    fn resolve_path_falls_back_to_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "root only").unwrap();

        let resolved = resolve_path(dir.path(), "SOUL.md").unwrap();
        assert_eq!(resolved, dir.path().join("SOUL.md"));
    }

    #[test]
    fn resolve_path_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        assert!(resolve_path(dir.path(), "SOUL.md").is_none());
    }
}
