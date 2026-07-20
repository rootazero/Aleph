//! `ToolUsageGrammarLayer` — encode tool usage conventions (priority 550)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ToolUsageGrammarLayer;

impl PromptLayer for ToolUsageGrammarLayer {
    fn name(&self) -> &'static str {
        "tool_usage_grammar"
    }
    fn priority(&self) -> u32 {
        550
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let tools = match input.tools {
            Some(t) => t,
            None => return,
        };

        let hints: Vec<_> = tools
            .iter()
            .filter_map(|t| t.usage_hint.as_ref().map(|h| (&t.name, h)))
            .collect();

        if hints.is_empty() {
            return;
        }

        output.push_str("## Tool Usage Guidelines\n\n");
        for (name, hint) in &hints {
            if !hint.prefer_over.is_empty() {
                let alternatives = hint.prefer_over.join(", ");
                if hint.prefer_for.is_empty() {
                    output.push_str(&format!("- Use `{name}` instead of {alternatives}\n"));
                } else {
                    for scenario in &hint.prefer_for {
                        output.push_str(&format!(
                            "- For {scenario}, use `{name}` instead of {alternatives}\n"
                        ));
                    }
                }
            }
        }
        output.push_str("- Prefer parallel tool calls when tasks are independent\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::tools::info::{ToolInfo, ToolUsageHint};

    #[test]
    fn skips_when_no_hints() {
        let layer = ToolUsageGrammarLayer;
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "t".into(),
            description: "d".into(),
            parameters_schema: None,
            usage_hint: None,
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn generates_grammar() {
        let layer = ToolUsageGrammarLayer;
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "file_read".into(),
            description: "Read".into(),
            parameters_schema: None,
            usage_hint: Some(ToolUsageHint {
                prefer_for: vec!["reading files".into()],
                prefer_over: vec!["cat".into(), "head".into()],
            }),
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool Usage Guidelines"));
        assert!(out.contains("file_read"));
        assert!(out.contains("cat"));
    }

    #[test]
    fn metadata() {
        let layer = ToolUsageGrammarLayer;
        assert_eq!(layer.name(), "tool_usage_grammar");
        assert_eq!(layer.priority(), 550);
    }
}
