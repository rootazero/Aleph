//! CustomInstructionsLayer — user-provided custom instructions (priority 1500)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct CustomInstructionsLayer;

impl PromptLayer for CustomInstructionsLayer {
    fn name(&self) -> &'static str { "custom_instructions" }
    fn priority(&self) -> u32 { 1500 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
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
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // If workspace IDENTITY.md exists, skip — handled by WorkspaceFilesLayer
        if input.workspace_file("IDENTITY.md").is_some() {
            return;
        }

        // Legacy fallback
        if let Some(instructions) = &input.config.custom_instructions {
            let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
            let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
            output.push_str("## Additional Instructions\n");
            output.push_str(&instructions);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_custom_instructions_present() {
        let layer = CustomInstructionsLayer;
        let config = PromptConfig {
            custom_instructions: Some("Always be concise.".to_string()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Additional Instructions"));
        assert!(out.contains("Always be concise."));
    }

    #[test]
    fn skips_when_workspace_identity_exists() {
        use crate::thinker::workspace_files::{WorkspaceFile, WorkspaceFiles};
        use std::path::PathBuf;

        let layer = CustomInstructionsLayer;
        let config = PromptConfig {
            custom_instructions: Some("Always be concise.".to_string()),
            ..Default::default()
        };
        let ws = WorkspaceFiles {
            root: PathBuf::from("/tmp"),
            files: vec![WorkspaceFile {
                name: "IDENTITY.md",
                content: "You are Aleph.".to_string(),
            }],
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_workspace(&ws);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty(), "Should skip when IDENTITY.md exists");
    }

    #[test]
    fn test_custom_instructions_absent() {
        let layer = CustomInstructionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
