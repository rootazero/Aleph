//! ProfileLayer — workspace persona overlay (priority 75)
//!
//! Injects the active workspace profile's system_prompt into the prompt
//! pipeline, between Soul (50) and Role (100).  This allows workspaces
//! to add role-specific context without overriding the base identity.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ProfileLayer;

impl PromptLayer for ProfileLayer {
    fn name(&self) -> &'static str { "profile" }
    fn priority(&self) -> u32 { 75 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // Priority 1: workspace AGENTS.md
        if let Some(agents_content) = input.workspace_file("AGENTS.md") {
            output.push_str("## Project Context\n\n");
            output.push_str(agents_content);
            output.push_str("\n\n");
            return;
        }

        // Priority 2: ProfileConfig.system_prompt (legacy fallback)
        let profile = match input.profile {
            Some(p) => p,
            None => return,
        };

        let prompt = match profile.system_prompt.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };

        output.push_str("## Current Role Context\n\n");
        output.push_str(prompt);
        output.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProfileConfig;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::soul::SoulManifest;
    use crate::thinker::workspace_files::{WorkspaceFile, WorkspaceFiles};
    use std::path::PathBuf;

    #[test]
    fn test_injects_when_prompt_exists() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: Some("You are a senior Rust engineer.".to_string()),
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Current Role Context"));
        assert!(out.contains("You are a senior Rust engineer."));
    }

    #[test]
    fn test_skips_when_no_prompt() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: None,
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_skips_when_no_profile() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let input = LayerInput::soul(&config, &tools, &soul);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_skips_when_prompt_is_empty() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: Some("".to_string()),
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn prefers_workspace_agents_over_profile_prompt() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: Some("You are a senior Rust engineer.".to_string()),
            ..Default::default()
        };
        let ws = WorkspaceFiles {
            workspace_dir: PathBuf::from("/tmp/test"),
            files: vec![WorkspaceFile {
                name: "AGENTS.md",
                content: Some("Custom agent instructions".to_string()),
                truncated: false,
                original_size: 24,
            }],
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile))
            .with_workspace(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Should use AGENTS.md, not ProfileConfig
        assert!(out.contains("## Project Context"));
        assert!(out.contains("Custom agent instructions"));
        assert!(!out.contains("## Current Role Context"));
        assert!(!out.contains("senior Rust engineer"));
    }

    #[test]
    fn falls_back_to_profile_when_no_workspace_agents() {
        let layer = ProfileLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let soul = SoulManifest::default();
        let profile = ProfileConfig {
            system_prompt: Some("You are a senior Rust engineer.".to_string()),
            ..Default::default()
        };
        // Workspace exists but has no AGENTS.md
        let ws = WorkspaceFiles {
            workspace_dir: PathBuf::from("/tmp/test"),
            files: vec![WorkspaceFile {
                name: "IDENTITY.md",
                content: Some("identity content".to_string()),
                truncated: false,
                original_size: 16,
            }],
        };
        let input = LayerInput::soul(&config, &tools, &soul)
            .with_profile(Some(&profile))
            .with_workspace(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Should fall back to ProfileConfig.system_prompt
        assert!(out.contains("## Current Role Context"));
        assert!(out.contains("You are a senior Rust engineer."));
        assert!(!out.contains("## Project Context"));
    }

    #[test]
    fn test_profile_paths() {
        let paths = ProfileLayer.paths();
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(!paths.contains(&AssemblyPath::Basic));
    }

    #[test]
    fn test_profile_priority() {
        assert_eq!(ProfileLayer.priority(), 75);
    }
}
