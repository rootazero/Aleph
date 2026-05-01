//! IdentityFilesLayer — inject remaining identity files (priority 1730)
//!
//! SOUL.md is handled by SoulLayer (priority 50), AGENTS.md by ProfileLayer
//! (priority 75). This layer injects the rest: IDENTITY.md, TOOLS.md,
//! HEARTBEAT.md. MEMORY.md is owned by `CuratedMemoryLayer` (Stable) and
//! never flows through this Dynamic layer.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// File names handled by dedicated layers — excluded from this layer.
const HANDLED_ELSEWHERE: &[&str] = &["SOUL.md", "AGENTS.md"];

pub struct IdentityFilesLayer;

impl PromptLayer for IdentityFilesLayer {
    fn name(&self) -> &'static str {
        "identity_files"
    }

    fn priority(&self) -> u32 {
        1730
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let identity = match input.identity_files {
            Some(files) => files,
            None => return,
        };

        let mut sections = Vec::new();

        for file in &identity.files {
            if HANDLED_ELSEWHERE.contains(&file.name) {
                continue;
            }
            if let Some(ref content) = file.content {
                sections.push(format!("### {}\n{}", file.name, content));
            }
        }

        if !sections.is_empty() {
            output.push_str("## Identity Files\n\n");
            output.push_str(&sections.join("\n\n"));
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::identity_files::{IdentityFile, IdentityFiles};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_mode::PromptMode;
    use std::path::PathBuf;

    fn make_identity(files: Vec<IdentityFile>) -> IdentityFiles {
        IdentityFiles {
            identity_dir: PathBuf::from("/tmp/test"),
            files,
        }
    }

    fn make_file(name: &'static str, content: &str) -> IdentityFile {
        IdentityFile {
            name,
            content: Some(content.to_string()),
            truncated: false,
            original_size: content.len(),
        }
    }

    fn make_empty_file(name: &'static str) -> IdentityFile {
        IdentityFile {
            name,
            content: None,
            truncated: false,
            original_size: 0,
        }
    }

    #[test]
    fn metadata() {
        let layer = IdentityFilesLayer;
        assert_eq!(layer.name(), "identity_files");
        assert_eq!(layer.priority(), 1730);
        assert_eq!(layer.paths().len(), 5);
        assert!(layer.paths().contains(&AssemblyPath::Basic));
        assert!(layer.paths().contains(&AssemblyPath::Soul));
        assert!(layer.paths().contains(&AssemblyPath::Cached));
    }

    #[test]
    fn supports_full_and_compact_not_minimal() {
        let layer = IdentityFilesLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn injects_remaining_files_excludes_soul_and_agents() {
        let layer = IdentityFilesLayer;
        let config = PromptConfig::default();

        let ws = make_identity(vec![
            make_file("SOUL.md", "soul content"),
            make_file("IDENTITY.md", "identity content"),
            make_file("AGENTS.md", "agents content"),
            make_file("TOOLS.md", "tools content"),
            make_file("HEARTBEAT.md", "heartbeat content"),
        ]);

        let input = LayerInput::basic(&config, &[]).with_identity_files(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Should contain the header
        assert!(out.contains("## Identity Files"));

        // Should include remaining files
        assert!(out.contains("### IDENTITY.md"));
        assert!(out.contains("identity content"));
        assert!(out.contains("### TOOLS.md"));
        assert!(out.contains("tools content"));
        assert!(out.contains("### HEARTBEAT.md"));
        assert!(out.contains("heartbeat content"));

        // Should NOT include SOUL.md or AGENTS.md
        assert!(!out.contains("### SOUL.md"));
        assert!(!out.contains("soul content"));
        assert!(!out.contains("### AGENTS.md"));
        assert!(!out.contains("agents content"));
    }

    #[test]
    fn skips_when_no_identity_files() {
        let layer = IdentityFilesLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn skips_files_with_no_content() {
        let layer = IdentityFilesLayer;
        let config = PromptConfig::default();

        let ws = make_identity(vec![
            make_empty_file("IDENTITY.md"),
            make_file("TOOLS.md", "has content"),
            make_empty_file("HEARTBEAT.md"),
        ]);

        let input = LayerInput::basic(&config, &[]).with_identity_files(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("### TOOLS.md"));
        assert!(!out.contains("### IDENTITY.md"));
        assert!(!out.contains("### HEARTBEAT.md"));
    }

    #[test]
    fn empty_when_all_files_missing_or_excluded() {
        let layer = IdentityFilesLayer;
        let config = PromptConfig::default();

        let ws = make_identity(vec![
            make_file("SOUL.md", "excluded"),
            make_file("AGENTS.md", "excluded"),
            make_empty_file("IDENTITY.md"),
        ]);

        let input = LayerInput::basic(&config, &[]).with_identity_files(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
