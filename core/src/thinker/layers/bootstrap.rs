//! Bootstrap file injection layer.
//!
//! Loads workspace-level context files (SOUL.md, IDENTITY.md, AGENTS.md, etc.)
//! and injects them into the system prompt with truncation management.

use std::path::PathBuf;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_budget::truncate_with_head_tail;

/// Ordered list of bootstrap files by priority (highest first).
const BOOTSTRAP_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
];

/// Layer that injects workspace bootstrap files into the system prompt.
pub struct BootstrapLayer {
    workspace: PathBuf,
    max_chars_per_file: usize,
    max_chars_total: usize,
}

impl BootstrapLayer {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            max_chars_per_file: 20_000,
            max_chars_total: 100_000,
        }
    }

    pub fn with_limits(mut self, per_file: usize, total: usize) -> Self {
        self.max_chars_per_file = per_file;
        self.max_chars_total = total;
        self
    }

    /// Load and format bootstrap files within budget.
    fn load_files(&self) -> Option<String> {
        let mut sections = Vec::new();
        let mut total_chars = 0;

        for &filename in BOOTSTRAP_FILES {
            if total_chars >= self.max_chars_total {
                break;
            }

            let path = self.resolve_path(filename);
            let content = match std::fs::read_to_string(&path) {
                Ok(c) if !c.trim().is_empty() => c,
                _ => continue,
            };

            // Per-file truncation
            let content = if content.len() > self.max_chars_per_file {
                truncate_with_head_tail(&content, self.max_chars_per_file, 0.7, 0.2)
            } else {
                content
            };

            // Total budget check
            let remaining = self.max_chars_total - total_chars;
            let content = if content.len() > remaining {
                truncate_with_head_tail(&content, remaining, 0.7, 0.2)
            } else {
                content
            };

            total_chars += content.len();
            sections.push(format!("### {}\n{}", filename, content));
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("## Workspace Context\n\n{}", sections.join("\n\n")))
        }
    }

    /// Resolve bootstrap file path. Check .aleph/ first, then workspace root.
    fn resolve_path(&self, filename: &str) -> PathBuf {
        let aleph_path = self.workspace.join(".aleph").join(filename);
        if aleph_path.exists() {
            return aleph_path;
        }
        self.workspace.join(filename)
    }
}

impl PromptLayer for BootstrapLayer {
    fn name(&self) -> &'static str { "bootstrap" }
    fn priority(&self) -> u32 { 55 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        if let Some(content) = self.load_files() {
            output.push_str(&content);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::fs;
    use tempfile::tempdir;

    fn create_bootstrap_file(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn layer_metadata() {
        let layer = BootstrapLayer::new(PathBuf::from("/tmp"));
        assert_eq!(layer.name(), "bootstrap");
        assert_eq!(layer.priority(), 55);
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn loads_existing_files() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "SOUL.md", "# Project\nRust AI assistant");
        create_bootstrap_file(dir.path(), "IDENTITY.md", "Always use Chinese");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("## Workspace Context"));
        assert!(content.contains("### SOUL.md"));
        assert!(content.contains("Rust AI assistant"));
        assert!(content.contains("### IDENTITY.md"));
        assert!(content.contains("Always use Chinese"));
    }

    #[test]
    fn skips_missing_files() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "SOUL.md", "Only soul file");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("SOUL.md"));
        assert!(!content.contains("IDENTITY.md"));
        assert!(!content.contains("TOOLS.md"));
    }

    #[test]
    fn returns_none_when_no_files() {
        let dir = tempdir().unwrap();
        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        assert!(layer.load_files().is_none());
    }

    #[test]
    fn truncates_large_files() {
        let dir = tempdir().unwrap();
        let large_content = "X".repeat(30_000);
        create_bootstrap_file(dir.path(), "SOUL.md", &large_content);

        let layer = BootstrapLayer::new(dir.path().to_path_buf())
            .with_limits(20_000, 100_000);
        let content = layer.load_files().unwrap();

        assert!(content.contains("[..."));
        assert!(content.len() < 30_000);
    }

    #[test]
    fn respects_total_budget() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "SOUL.md", &"A".repeat(80_000));
        create_bootstrap_file(dir.path(), "IDENTITY.md", &"B".repeat(80_000));

        let layer = BootstrapLayer::new(dir.path().to_path_buf())
            .with_limits(80_000, 100_000);
        let content = layer.load_files().unwrap();

        // Total should be around 100K, not 160K
        assert!(content.len() <= 110_000);
    }

    #[test]
    fn loads_heartbeat_and_agents_files() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "HEARTBEAT.md", "# Heartbeat\nSystem status: healthy");
        create_bootstrap_file(dir.path(), "AGENTS.md", "# Operating Manual\nAlways run tests before committing");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("### HEARTBEAT.md"));
        assert!(content.contains("System status"));
        assert!(content.contains("### AGENTS.md"));
        assert!(content.contains("Always run tests"));
    }

    #[test]
    fn prefers_aleph_dir() {
        let dir = tempdir().unwrap();
        create_bootstrap_file(dir.path(), "SOUL.md", "root version");
        fs::create_dir_all(dir.path().join(".aleph")).unwrap();
        create_bootstrap_file(&dir.path().join(".aleph"), "SOUL.md", "aleph version");

        let layer = BootstrapLayer::new(dir.path().to_path_buf());
        let content = layer.load_files().unwrap();

        assert!(content.contains("aleph version"));
        assert!(!content.contains("root version"));
    }
}
