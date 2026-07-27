//! `StrategyLayer` — emits the welded `<strategy>` envelope at priority 70
//! (Stable, cacheable prefix).
//!
//! The StraTA-pattern strategic plan, minted once per long task by the
//! planner node and pinned into the stable, prefix-cacheable head of the
//! system prompt so its KV-cache is reused across every turn ("draw the map before
//! you start; don't forget why you began"). Sits between `CuratedMemoryLayer` (60) and
//! `ProfileLayer` (75) in the Stable zone.
//!
//! R10-safe: pure scaffolding. The body is the planner LLM's own rendered
//! `Strategy`, injected verbatim — the harness makes no judgment, runs no
//! extra LLM call here, and applies no relevance scoring. The content is
//! rendered once (deterministically, no timestamps) by `render_strategy_summary`
//! and stored in `ResolvedContext.strategy`. `None` emits nothing, leaving
//! the cacheable prefix byte-identical for sessions with no Strategy.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct StrategyLayer;

impl PromptLayer for StrategyLayer {
    fn name(&self) -> &'static str {
        "strategy"
    }

    fn priority(&self) -> u32 {
        70
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn stability(&self) -> LayerStability {
        // The welded Strategy is minted once per task and held verbatim across
        // every turn — Stable so it rides the cached stable prefix.
        LayerStability::Stable
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // The full <strategy> body is operational steering, not core framing —
        // drop it from the bare Minimal prompt, matching StrategyPointerLayer
        // (which echoes the same plan's guardrails) so the two strategy
        // surfaces stay symmetric: both present in Full/Compact, both absent in
        // Minimal. (Mirrors StandingGoal / ExecutionPlan.)
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(strategy) = ctx.strategy.as_deref() else {
            return;
        };
        if strategy.is_empty() {
            return;
        }
        output.push_str("<strategy>\n");
        // Escaped at the seam: objective / phases / guardrails are authored from
        // user input, so an unescaped closing tag would break out of this element
        // and forge top-level prompt sections. Single source: `xml_util`.
        output.push_str(&crate::thinker::xml_util::escape_xml(strategy));
        output.push_str("\n</strategy>\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_with_strategy(strategy: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.strategy = strategy.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        StrategyLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn no_strategy_emits_nothing() {
        let out = render(&ctx_with_strategy(None));
        assert!(out.is_empty());
    }

    #[test]
    fn empty_strategy_emits_nothing() {
        // present-but-empty body must still leave the prompt byte-identical.
        let out = render(&ctx_with_strategy(Some("")));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        StrategyLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn strategy_renders_inside_tag() {
        let body = "Objective: ship the planner\nApproach: plan-first\nGuardrails:\n- don't refactor unrelated modules";
        let out = render(&ctx_with_strategy(Some(body)));
        assert!(out.starts_with("<strategy>\n"));
        assert!(out.contains("Objective: ship the planner"));
        assert!(out.contains("don't refactor unrelated modules"));
        assert!(out.trim_end().ends_with("</strategy>"));
    }

    #[test]
    fn name_priority_stability() {
        assert_eq!(StrategyLayer.name(), "strategy");
        assert_eq!(StrategyLayer.priority(), 70);
        assert_eq!(StrategyLayer.stability(), LayerStability::Stable);
        assert!(StrategyLayer.paths().contains(&AssemblyPath::Cached));
    }
}
