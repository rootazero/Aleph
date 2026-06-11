//! SessionBudgetLayer — iteration-cap awareness + self-tracking protocol
//! (priority 820).
//!
//! Surfaces the per-run Think→Act iteration cap so the LLM can plan
//! tool-call cadence around it. The cap is *static* — fixed when the
//! run starts — which makes the layer safe to emit as part of the
//! cacheable Stable prefix.
//!
//! # Phase 6 — self-tracking protocol
//!
//! True per-turn dynamic signals ("you have N turns left right now")
//! would require modifying the harness Think loop to inject ephemeral
//! per-turn content. `src/harness/` is already at the R10 budget
//! ceiling, so we route the signal through the LLM instead: the layer
//! tells the model HOW to count its own progress from the conversation
//! history (R7 LLM Sovereignty) and WHAT to do as it approaches the
//! cap. The static prompt becomes a self-aware protocol the LLM
//! applies turn-by-turn at zero harness cost.
//!
//! Stability: Stable. Mode: Full only.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct SessionBudgetLayer;

impl PromptLayer for SessionBudgetLayer {
    fn name(&self) -> &'static str {
        "session_budget"
    }

    fn priority(&self) -> u32 {
        // Sits after `ProviderGuidanceLayer` (810) and before
        // `CitationStandardsLayer` (900): the iteration budget is
        // operational context, not citation discipline.
        820
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Stable
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

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let cap = match input.iteration_cap {
            Some(n) if n > 0 => n,
            _ => return,
        };
        // Tier thresholds: pre-computed so the prompt carries concrete
        // turn numbers, not arithmetic the LLM has to redo each turn.
        // 75 % → checkpoint, 90 % → closure-mode trigger.
        let checkpoint_at = (cap as f32 * 0.75).ceil() as u32;
        let closure_at = (cap as f32 * 0.90).ceil() as u32;

        output.push_str("## Session Budget\n\n");
        output.push_str(&format!(
            "- **Iteration cap**: {cap} — the Think→Act loop is forced to wrap up after this many turns.\n"
        ));
        output.push_str(
            "- Front-load the most decisive action — at the cap the harness emits a final reply regardless of progress.\n",
        );
        output.push_str("\n### Self-pacing protocol\n\n");
        output.push_str(
            "Track your turn count from the conversation history (each assistant message = one turn):\n\n",
        );
        output.push_str(&format!(
            "- **Turn ≤ {checkpoint_at} (≤ 75 %)** — explore: branch on hypotheses, run tools that surface information.\n"
        ));
        output.push_str(&format!(
            "- **Turn {} – {closure_at} (75–90 %)** — checkpoint: reconcile findings, drop non-converging branches, prefer gap-closing tools over gap-opening ones.\n",
            checkpoint_at.saturating_add(1)
        ));
        output.push_str(&format!(
            "- **Turn > {closure_at} (> 90 %)** — closure: no new exploration; synthesize and deliver a final reply next turn, flagging any unresolved items.\n"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata_and_priority() {
        let layer = SessionBudgetLayer;
        assert_eq!(layer.name(), "session_budget");
        assert_eq!(layer.priority(), 820);
        assert!(matches!(layer.stability(), LayerStability::Stable));
    }

    #[test]
    fn supports_full_only() {
        let layer = SessionBudgetLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn participates_in_every_non_minimal_path() {
        let paths = SessionBudgetLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn silent_when_cap_missing() {
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn silent_when_cap_zero() {
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_iteration_cap(0);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.is_empty(),
            "zero cap should be treated as unset (consistent with resolve_max_iterations)"
        );
    }

    #[test]
    fn emits_block_when_cap_set() {
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_iteration_cap(42);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Session Budget"));
        assert!(out.contains("Iteration cap**: 42"));
        assert!(out.contains("decisive action"));
    }

    #[test]
    fn cap_one_still_emits() {
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_iteration_cap(1);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("Iteration cap**: 1"));
    }

    #[test]
    fn emits_self_pacing_protocol_with_concrete_tier_thresholds() {
        // Cap 20 → 75 % = 15, 90 % = 18. Layer must carry those exact
        // numbers so the LLM doesn't redo the arithmetic each turn.
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_iteration_cap(20);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("### Self-pacing protocol"));
        assert!(out.contains("conversation history"));
        // 75 % of 20 = 15. Turn ≤ 15 is exploratory.
        assert!(
            out.contains("Turn ≤ 15"),
            "checkpoint threshold should land at ceil(20 * 0.75) = 15"
        );
        // 90 % of 20 = 18. Turn > 18 is closure.
        assert!(
            out.contains("Turn > 18"),
            "closure threshold should land at ceil(20 * 0.90) = 18"
        );
        // Checkpoint phase opens at 16 (= 15 + 1).
        assert!(
            out.contains("Turn 16 – 18"),
            "checkpoint phase should span 16..=18"
        );
    }

    #[test]
    fn self_pacing_protocol_scales_with_cap() {
        // Cap 200 → 75 % = 150, 90 % = 180.
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_iteration_cap(200);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("Turn ≤ 150"));
        assert!(out.contains("Turn > 180"));
        assert!(out.contains("Turn 151 – 180"));
    }
}
