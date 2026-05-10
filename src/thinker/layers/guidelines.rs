//! GuidelinesLayer — general operational guidelines (priority 1300)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct GuidelinesLayer;

impl PromptLayer for GuidelinesLayer {
    fn name(&self) -> &'static str {
        "guidelines"
    }
    fn priority(&self) -> u32 {
        1300
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
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
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Guidelines\n");
        output.push_str("1. Take ONE action at a time, observe the result, then decide next\n");
        output.push_str("2. Use tool results to inform subsequent decisions\n");
        output.push_str(
            "3. Ask user when: multiple valid approaches, unclear requirements, need confirmation\n",
        );
        output.push_str(
            "4. Complete when: task is done, or you've provided the requested information\n",
        );
        output.push_str("5. Fail when: impossible to proceed, missing critical resources\n");
        output.push_str("6. Always close the loop — every turn ends with a reply to your caller; never finish on a tool call alone\n");
        output.push_str("7. 2-strike rule — if the same class of error repeats twice (same path missing, same query empty, same command failing), stop and report what you tried plus your best guess at the cause\n");
        output.push_str("8. Subagent error/timeout → report, never spawn replacement subagents on your own; summarize all sub-results (success + errors + timeouts) back to your caller\n");
        output.push_str("9. Suspected typo > silent search — if a caller-supplied path/identifier doesn't exist on first lookup, list nearby candidates and ask, don't fan out tool calls guessing\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_guidelines_content() {
        let layer = GuidelinesLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Guidelines"));
        assert!(out.contains("Take ONE action at a time"));
        assert!(out.contains("Fail when: impossible to proceed"));
        // Fail-fast bullets (Phase-3): close-the-loop, 2-strike, subagent
        // error escalation, typo-first. These extend the layer to make
        // every agent's loop terminate with a user-visible reply instead
        // of silently looping on bad input.
        assert!(out.contains("Always close the loop"));
        assert!(out.contains("2-strike rule"));
        assert!(out.contains("Subagent error/timeout"));
        assert!(out.contains("Suspected typo"));
    }

    #[test]
    fn test_guidelines_priority_and_paths() {
        assert_eq!(GuidelinesLayer.priority(), 1300);
        let paths = GuidelinesLayer.paths();
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
