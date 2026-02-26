//! SpecialActionsLayer — special action definitions (priority 1100)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SpecialActionsLayer;

impl PromptLayer for SpecialActionsLayer {
    fn name(&self) -> &'static str { "special_actions" }
    fn priority(&self) -> u32 { 1100 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Special Actions\n");
        output.push_str("- `complete`: Call when the task is fully done. The `summary` field MUST be a comprehensive report that includes:\n");
        output.push_str("  1. A brief overview of what was accomplished\n");
        output.push_str("  2. Key results and findings (data, insights, metrics)\n");
        output.push_str("  3. List of all generated files with their purposes\n");
        output.push_str("  4. Any important notes or recommendations\n");
        output.push_str(
            "  **DO NOT** just say 'Task completed'. Write a detailed summary the user can immediately understand.\n",
        );
        output.push_str("- `ask_user`: Call when you need clarification or user decision\n");
        output.push_str("- `fail`: Call when the task cannot be completed\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_special_actions_content() {
        let layer = SpecialActionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Special Actions"));
        assert!(out.contains("`complete`"));
        assert!(out.contains("`ask_user`"));
        assert!(out.contains("`fail`"));
        assert!(out.contains("DO NOT"));
    }

    #[test]
    fn test_special_actions_priority() {
        assert_eq!(SpecialActionsLayer.priority(), 1100);
    }
}
