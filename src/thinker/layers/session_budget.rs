//! SessionBudgetLayer — static iteration-cap awareness (priority 820).
//!
//! Surfaces the per-run Think→Act iteration cap so the LLM can plan
//! tool-call cadence around it. The cap is *static* — it is fixed when
//! the run starts and does not change mid-run, which makes the layer
//! safe to emit as part of the cacheable Stable prefix.
//!
//! Phase 4 deliberately keeps this scope narrow: per-turn pressure
//! signals (e.g. "you have N turns left") would require a per-turn
//! injection channel that does not exist yet (the system prompt is
//! built once per run). The static cap is the cacheable, low-risk
//! portion of that idea.
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
        output.push_str("## Session Budget\n\n");
        output.push_str(&format!(
            "- **Iteration cap**: {} (the Think→Act loop is forced to wrap up after this many turns).\n",
            cap
        ));
        output.push_str(
            "- Plan your tool calls so the most decisive action lands early — once the cap is reached, the harness emits a final reply regardless of progress.\n",
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
}
