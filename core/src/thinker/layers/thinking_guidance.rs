//! ThinkingGuidanceLayer — structured reasoning transparency (priority 1350)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ThinkingGuidanceLayer;

impl PromptLayer for ThinkingGuidanceLayer {
    fn name(&self) -> &'static str { "thinking_guidance" }
    fn priority(&self) -> u32 { 1350 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if !input.config.thinking_transparency {
            return;
        }

        output.push_str("## Thinking Transparency\n\n");
        output.push_str("Structure your reasoning to be transparent and understandable:\n\n");

        output.push_str("### Reasoning Flow\n");
        output.push_str("Follow this progression in your `reasoning` field:\n\n");
        output.push_str("1. **Observation** (👁️): Start by observing the current state\n");
        output.push_str("   - \"Looking at the request, I see...\"\n");
        output.push_str("   - \"The user wants to...\"\n");
        output.push_str("   - \"Based on the previous result...\"\n\n");
        output.push_str("2. **Analysis** (🔍): Analyze options and trade-offs\n");
        output.push_str("   - \"Considering the options: A vs B vs C...\"\n");
        output.push_str("   - \"The trade-off here is...\"\n");
        output.push_str("   - \"Comparing approaches...\"\n\n");
        output.push_str("3. **Planning** (📝): Outline your approach\n");
        output.push_str("   - \"I'll start by...\"\n");
        output.push_str("   - \"First, ... then...\"\n");
        output.push_str("   - \"My strategy is to...\"\n\n");
        output.push_str("4. **Decision** (✅): State your conclusion\n");
        output.push_str("   - \"Therefore, I will...\"\n");
        output.push_str("   - \"The best approach is...\"\n");
        output.push_str("   - \"So I've decided to...\"\n\n");

        output.push_str("### Expressing Uncertainty\n");
        output.push_str("When uncertain, be explicit rather than hiding it:\n\n");
        output.push_str("- **High confidence**: \"I'm confident that...\" or \"Clearly,...\"\n");
        output.push_str("- **Medium confidence**: \"I think...\" or \"This should work...\"\n");
        output.push_str("- **Low confidence**: \"I'm not sure, but...\" or \"This might...\"\n");
        output.push_str("- **Exploratory**: \"Let's try...\" or \"Worth experimenting with...\"\n\n");

        output.push_str("### Acknowledging Alternatives\n");
        output.push_str("When relevant, mention alternatives you considered:\n");
        output.push_str("- \"Alternatively, we could...\"\n");
        output.push_str("- \"Another option would be...\"\n");
        output.push_str("- \"I chose X over Y because...\"\n\n");

        output.push_str("This structured thinking helps users understand your reasoning process.\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_thinking_guidance_active() {
        let layer = ThinkingGuidanceLayer;
        let config = PromptConfig {
            thinking_transparency: true,
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Thinking Transparency"));
        assert!(out.contains("Reasoning Flow"));
        assert!(out.contains("Expressing Uncertainty"));
        assert!(out.contains("Acknowledging Alternatives"));
    }

    #[test]
    fn test_thinking_guidance_inactive() {
        let layer = ThinkingGuidanceLayer;
        let config = PromptConfig::default(); // thinking_transparency = false
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
