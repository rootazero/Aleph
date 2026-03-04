//! RoleLayer — core role definition (priority 100)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct RoleLayer;

impl PromptLayer for RoleLayer {
    fn name(&self) -> &'static str { "role" }
    fn priority(&self) -> u32 { 100 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul, AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("You are an AI assistant executing tasks step by step.\n\n");

        output.push_str("## Your Role\n");
        output.push_str("- Observe the current state and history\n");
        output.push_str("- Decide the SINGLE next action to take\n");
        output.push_str("- Execute until the task is complete or you need user input\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_role_content() {
        let layer = RoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("You are an AI assistant executing tasks step by step."));
        assert!(out.contains("## Your Role"));
        assert!(out.contains("Observe the current state and history"));
        assert!(out.contains("SINGLE next action"));
    }

    #[test]
    fn test_role_priority() {
        assert_eq!(RoleLayer.priority(), 100);
    }

    #[test]
    fn test_role_paths() {
        let paths = RoleLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(!paths.contains(&AssemblyPath::Cached));
    }
}
