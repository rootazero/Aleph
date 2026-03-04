//! SkillInstructionsLayer — skill system v2 instructions (priority 1050)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct SkillInstructionsLayer;

impl PromptLayer for SkillInstructionsLayer {
    fn name(&self) -> &'static str { "skill_instructions" }
    fn priority(&self) -> u32 { 1050 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref instructions) = input.config.skill_instructions {
            if !instructions.is_empty() {
                let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
                let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
                output.push_str("## Available Skills\n\n");
                output.push_str("You can invoke skills using the `skill` tool. ");
                output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
                output.push_str(&instructions);
                output.push_str("\n\n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skill_instructions_present() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            skill_instructions: Some("<skill name=\"test\">Do something</skill>".to_string()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Available Skills"));
        assert!(out.contains("skill` tool"));
        assert!(out.contains("<skill name=\"test\">"));
    }

    #[test]
    fn test_skill_instructions_empty_string() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig {
            skill_instructions: Some(String::new()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_skill_instructions_absent() {
        let layer = SkillInstructionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_skill_instructions_paths() {
        let paths = SkillInstructionsLayer.paths();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
    }
}
