//! `StrategyPointerLayer` — re-echoes the Strategy's guardrails verbatim as
//! `<strategy_reminder>` at priority 1757 (Dynamic), near the read head.
//!
//! The Stable `StrategyLayer` (70) pins the full plan in the cacheable head,
//! but on a long horizon the head scrolls far from the model's read position.
//! This layer restates **only** the 1-3 concrete guardrails near the prompt
//! tail every turn — the operation drift already fails at — so the concrete
//! anti-distraction constraints stay salient. It deliberately omits the
//! objective: `StandingGoalLayer` (1755) already re-injects that for `/goal`,
//! and three near-identical end-of-prompt reminders breed reminder-blindness.
//!
//! R10-safe: pure scaffolding, guardrails injected verbatim, no judgment.
//! `Dynamic` keeps it out of the cached stable prefix; `None` (no Strategy or
//! no guardrails) emits nothing, leaving the dynamic tail byte-identical.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct StrategyPointerLayer;

impl PromptLayer for StrategyPointerLayer {
    fn name(&self) -> &'static str {
        "strategy_pointer"
    }

    fn priority(&self) -> u32 {
        1757
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn stability(&self) -> LayerStability {
        // The guardrail echo rides the per-turn dynamic suffix so it never
        // invalidates the cached stable prefix (which already holds the full
        // `<strategy>` via StrategyLayer).
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // Operational steering, not chrome — drop only from the bare Minimal
        // prompt, matching StandingGoal / ExecutionPlan.
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(guardrails) = ctx.strategy_guardrails.as_deref() else {
            return;
        };
        if guardrails.is_empty() {
            return;
        }
        output.push_str("<strategy_reminder>\n");
        // Escaped at the seam: guardrails are authored from user input, so an
        // unescaped closing tag would break out of this element and forge
        // top-level prompt sections. Single source: `xml_util`.
        output.push_str(&crate::thinker::xml_util::escape_xml(guardrails));
        output.push_str("\n</strategy_reminder>\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::prompt_mode::PromptMode;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_with_guardrails(guardrails: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
        );
        ctx.strategy_guardrails = guardrails.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        StrategyPointerLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn no_strategy_emits_nothing() {
        let out = render(&ctx_with_guardrails(None));
        assert!(out.is_empty());
    }

    #[test]
    fn empty_strategy_emits_nothing() {
        let out = render(&ctx_with_guardrails(Some("")));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        StrategyPointerLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn guardrails_render_inside_tag() {
        let guardrails =
            "- don't refactor unrelated modules\n- don't add config beyond what's asked";
        let out = render(&ctx_with_guardrails(Some(guardrails)));
        assert!(out.starts_with("<strategy_reminder>\n"));
        assert!(out.contains("don't refactor unrelated modules"));
        assert!(out.trim_end().ends_with("</strategy_reminder>"));
    }

    #[test]
    fn excluded_from_minimal_mode() {
        assert!(!StrategyPointerLayer.supports_mode(PromptMode::Minimal));
        assert!(StrategyPointerLayer.supports_mode(PromptMode::Full));
    }

    #[test]
    fn name_priority_stability() {
        assert_eq!(StrategyPointerLayer.name(), "strategy_pointer");
        assert_eq!(StrategyPointerLayer.priority(), 1757);
        assert_eq!(StrategyPointerLayer.stability(), LayerStability::Dynamic);
        assert!(StrategyPointerLayer.paths().contains(&AssemblyPath::Cached));
    }
}
