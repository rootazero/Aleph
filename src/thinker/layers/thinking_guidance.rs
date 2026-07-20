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

        // Lean: the Observation→Analysis→Planning→Decision framework + the
        // confidence/alternatives sample phrasings were a reasoning cage a
        // capable model doesn't need (§1.1 prune-the-prompt). Keep only the
        // directive. (Self-gated on `thinking_transparency`, off by default.)
        output.push_str(
            "## Thinking Transparency\n\nMake your reasoning visible as you work — what you \
             observe, the options you weigh, and why you choose one — and state your confidence \
             plainly rather than hiding it.\n\n",
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
        assert!(out.contains("reasoning visible"));
        assert!(out.contains("confidence"));
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
