//! GuidelinesLayer — general operational guidelines (priority 1300)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct GuidelinesLayer;

impl PromptLayer for GuidelinesLayer {
    fn name(&self) -> &'static str { "guidelines" }
    fn priority(&self) -> u32 { 1300 }
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
        output.push_str("5. Fail when: impossible to proceed, missing critical resources\n\n");
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
    }

    #[test]
    fn test_guidelines_priority_and_paths() {
        assert_eq!(GuidelinesLayer.priority(), 1300);
        let paths = GuidelinesLayer.paths();
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
