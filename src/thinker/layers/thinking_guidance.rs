//! `ThinkingGuidanceLayer` — structured reasoning transparency (priority 1350)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ThinkingGuidanceLayer;

impl PromptLayer for ThinkingGuidanceLayer {
    fn name(&self) -> &'static str {
        "thinking_guidance"
    }
    fn priority(&self) -> u32 {
        1350
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // `Cached` is the live main-loop path
        // (`build_system_prompt_cached_with_mode`). Without it this guidance
        // would silently never reach a production prompt even if
        // `thinking_transparency` were wired on — the same latent-vanish trap
        // the Soul / Role / Citation layers were fixed for. The layer is
        // Stable + Full-only and self-gates on the config flag, so riding the
        // cacheable prefix costs nothing when the flag is false (the current
        // default).
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if !input.config.thinking_transparency {
            return;
        }

        output.push_str("## Thinking Transparency\n\n");
        output.push_str("Make your reasoning visible as you work so the user can follow it:\n\n");
        output.push_str(
            "- **Reasoning Flow**: progress through Observation (current state) → \
             Analysis (options and trade-offs) → Planning (your approach) → Decision \
             (the conclusion you act on).\n",
        );
        output.push_str(
            "- **Expressing Uncertainty**: state your confidence plainly (\"I'm confident…\", \
             \"I think…\", \"I'm not sure, but…\") rather than hiding it.\n",
        );
        output.push_str(
            "- **Acknowledging Alternatives**: when relevant, name the options you weighed and \
             why you chose one (\"I chose X over Y because…\").\n\n",
        );
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
