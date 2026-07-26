//! `ProviderGuidanceLayer` — per-family operational delta (priority 810).
//!
//! Dispatches on the resolved governance **behavior name**
//! (`resolve_behavior`: anthropic / openai / gemini / ollama / strict /
//! unknown). The layer no longer hardcodes any shared "how to use tools / how
//! to persist" baseline: a frontier model does that natively and Aleph is
//! Claude-centric, so authoring those manuals in-prompt was a cage the strong
//! model doesn't need (§1.1 prune-the-prompt, R7/R9). All per-family steering
//! now lives entirely in the overridable `.md` delta threaded via
//! `LayerInput::model_behavior_delta` (`model_behaviors/{behavior}.md`,
//! user-overridable at `~/.aleph/model_behaviors/`) — so a weak family that
//! genuinely needs remedial coaching carries it in its own file, not baked
//! into every prompt (including Claude's).
//!
//! Stability: Stable (behavior is fixed per run; cacheable in the
//! prompt prefix).
//! Mode: Full only.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ProviderGuidanceLayer;

impl PromptLayer for ProviderGuidanceLayer {
    fn name(&self) -> &'static str {
        "provider_guidance"
    }

    fn priority(&self) -> u32 {
        // Sits immediately after OperationalGuidelinesLayer (800) so
        // family-specific directives layer on top of the paradigm
        // baseline.
        810
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Stable
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        // `behavior_name` gates whether this layer speaks at all: None =
        // capture / snapshot / tests → stay silent. When present, the only
        // content is the per-family `.md` delta (empty for Claude by default);
        // the old hardcoded tool-use / persistence baselines were removed as a
        // cage a capable model doesn't need (§1.1 prune-the-prompt).
        if input.behavior_name.is_none() {
            return;
        }
        if let Some(delta) = input.model_behavior_delta {
            let delta = delta.trim();
            if !delta.is_empty() {
                output.push_str(delta);
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
    fn metadata_and_priority() {
        let layer = ProviderGuidanceLayer;
        assert_eq!(layer.name(), "provider_guidance");
        assert_eq!(layer.priority(), 810);
        assert!(matches!(layer.stability(), LayerStability::Stable));
    }

    #[test]
    fn supports_full_only() {
        let layer = ProviderGuidanceLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn participates_in_every_non_minimal_path() {
        let paths = ProviderGuidanceLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn silent_when_behavior_missing() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn emits_only_the_family_delta() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];

        // A behavior with no delta → silent: no hardcoded baseline prose.
        let input = LayerInput::basic(&config, &tools).with_behavior_name_opt(Some("anthropic"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty(), "no delta → no hardcoded baseline");

        // A behavior with a delta → exactly the delta, and none of the old
        // hardcoded tool-use / persistence baselines.
        let delta = "## Execution Discipline — OpenAI Family\nAct, don't ask";
        let input = LayerInput::basic(&config, &tools)
            .with_behavior_name_opt(Some("openai"))
            .with_model_behavior_delta_opt(Some(delta));
        out.clear();
        layer.inject(&mut out, &input);
        assert!(out.contains("Act, don't ask"), "family delta must appear");
        assert!(!out.contains("## Tool-Use Enforcement"));
        assert!(!out.contains("## Execution Discipline — Persistence"));
    }
}
