//! EnvironmentLayer — environment contract injection (priority 300)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct EnvironmentLayer;

impl PromptLayer for EnvironmentLayer {
    fn name(&self) -> &'static str { "environment" }
    fn priority(&self) -> u32 { 300 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        let contract = &ctx.environment_contract;

        output.push_str("## Environment Contract\n\n");

        // Paradigm description
        output.push_str(&format!(
            "**Paradigm**: {}\n\n",
            contract.paradigm.description()
        ));

        // Active capabilities
        if !contract.active_capabilities.is_empty() {
            output.push_str("**Active Capabilities**:\n");
            for cap in &contract.active_capabilities {
                let (name, hint) = cap.prompt_hint();
                output.push_str(&format!("- `{}`: {}\n", name, hint));
            }
            output.push('\n');
        }

        // Constraints
        let mut constraint_notes = Vec::new();
        if let Some(max_chars) = contract.constraints.max_output_chars {
            constraint_notes.push(format!("Max output: {} characters", max_chars));
        }
        if contract.constraints.prefer_compact {
            constraint_notes.push("Prefer concise responses".to_string());
        }
        if contract.constraints.supports_streaming {
            constraint_notes.push("Streaming enabled".to_string());
        }

        if !constraint_notes.is_empty() {
            output.push_str("**Constraints**:\n");
            for note in constraint_notes {
                output.push_str(&format!("- {}\n", note));
            }
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_environment_no_context() {
        let layer = EnvironmentLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools); // no context
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_environment_paths() {
        let paths = EnvironmentLayer.paths();
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&AssemblyPath::Context));
    }
}
