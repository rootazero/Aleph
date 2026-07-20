//! `SessionBudgetLayer` — iteration-cap awareness + self-tracking protocol
//! (priority 820).
//!
//! Surfaces the per-run Think→Act iteration cap so the LLM can plan
//! tool-call cadence around it. The cap is *static* — fixed when the
//! run starts — which makes the layer safe to emit as part of the
//! cacheable Stable prefix.
//!
//! # Self-pacing
//!
//! The layer surfaces the cap number plus a one-line "pace toward a final
//! answer before it" nudge. It deliberately does NOT teach a tiered turn-
//! counting manual (explore / checkpoint / closure with pre-computed
//! thresholds): that was how-to-think-in-prose a capable model does natively,
//! cut under §1.1 prune-the-prompt (R7/R9). True per-turn "N turns left"
//! signals would need harness changes, and `src/harness/` is at the R10
//! ceiling anyway.
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
            AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let cap = match input.iteration_cap {
            Some(n) if n > 0 => n,
            _ => return,
        };
        output.push_str("## Session Budget\n\n");
        output.push_str(&format!(
            "- **Iteration cap**: {cap} — the Think→Act loop is forced to wrap up after this many turns.\n"
        ));
        output.push_str(
            "- Front-load the most decisive action and pace yourself toward a final answer before the cap; at the cap the harness emits a final reply regardless of progress.\n",
        );
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
    fn no_tiered_self_pacing_manual() {
        // §1.1 prune-the-prompt: the tiered explore/checkpoint/closure manual
        // with pre-computed thresholds was cut — only the cap number + a one-
        // line pacing nudge remain.
        let layer = SessionBudgetLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_iteration_cap(20);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("Iteration cap**: 20"));
        assert!(!out.contains("Self-pacing protocol"));
        assert!(!out.contains("Turn ≤"));
        assert!(!out.contains("checkpoint"));
    }
}
