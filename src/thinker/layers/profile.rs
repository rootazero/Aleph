//! `ProfileLayer` — workspace persona overlay (priority 75)
//!
//! Injects the workspace `AGENTS.md` as a `## Project Context` block into the
//! prompt pipeline, between Soul (50) and Role (100). This lets a workspace add
//! project-specific context without overriding the base identity.

use super::identity_files::sanitize_identity_content;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ProfileLayer;

impl PromptLayer for ProfileLayer {
    fn name(&self) -> &'static str {
        "profile"
    }
    fn priority(&self) -> u32 {
        75
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // `Cached` is the live main-loop path
        // (`build_system_prompt_cached_with_mode`). Without it, AGENTS.md —
        // which `IdentityFilesLayer` defers here via `HANDLED_ELSEWHERE` —
        // would vanish from every production prompt (same class of bug the
        // Role / Citation layers were fixed for).
        &[AssemblyPath::Cached]
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Stable
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // Workspace AGENTS.md → "## Project Context" overlay. It crosses the
        // same user-editable trust boundary as the other identity files, so it
        // gets the same injection-pattern + invisible-Unicode scan SoulLayer
        // applies to SOUL.md and IdentityFilesLayer applies to IDENTITY.md /
        // TOOLS.md / HEARTBEAT.md — otherwise AGENTS.md would be a one-file
        // bypass of that hardening.
        if let Some(agents_content) = input.identity_file("AGENTS.md") {
            let safe = sanitize_identity_content("AGENTS.md", agents_content);
            output.push_str("## Project Context\n\n");
            output.push_str(&safe);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::identity_files::{IdentityFile, IdentityFiles};
    use crate::thinker::prompt_builder::PromptConfig;
    use std::path::PathBuf;

    fn workspace_with(name: &'static str, content: &str) -> IdentityFiles {
        IdentityFiles {
            identity_dir: PathBuf::from("/tmp/test"),
            files: vec![IdentityFile {
                name,
                content: Some(content.to_string()),
                truncated: false,
                original_size: content.len(),
            }],
        }
    }

    #[test]
    fn injects_agents_md_as_project_context() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let ws = workspace_with("AGENTS.md", "Custom agent instructions");
        let input = LayerInput::basic(&config, &[]).with_identity_files(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Project Context"));
        assert!(out.contains("Custom agent instructions"));
    }

    #[test]
    fn skips_when_no_agents_md() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        // Workspace exists but carries no AGENTS.md → nothing to inject.
        let ws = workspace_with("IDENTITY.md", "identity content");
        let input = LayerInput::basic(&config, &[]).with_identity_files(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_no_identity_files() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_profile_paths() {
        let paths = ProfileLayer.paths();
        assert!(!paths.contains(&AssemblyPath::Basic));
        // Must ride the live main-loop path so AGENTS.md actually reaches the
        // production prompt.
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn test_profile_priority() {
        assert_eq!(ProfileLayer.priority(), 75);
    }
}
