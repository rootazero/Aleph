//! `StandingGoalLayer` — emits `<standing_goal>` at priority 1755 (Dynamic).
//!
//! Re-surfaces the session's active standing goal into the system prompt
//! every turn while it is active — the cross-turn complement to
//! `ExecutionPlanLayer` (1756, per-task checklist). hermes-agent re-states
//! the goal in every continuation; this is Aleph's R10-safe equivalent: pure
//! scaffolding, the content is the user's own objective + the goal's own
//! status, rendered verbatim. No judgment, no LLM call. `None` emits nothing,
//! leaving the prompt byte-identical for sessions with no standing goal.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct StandingGoalLayer;

impl PromptLayer for StandingGoalLayer {
    fn name(&self) -> &'static str {
        "standing_goal"
    }

    fn priority(&self) -> u32 {
        1755
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(goal) = ctx.standing_goal.as_deref() else {
            return;
        };
        if goal.is_empty() {
            return;
        }
        output.push_str("<standing_goal>\n");
        // Escaped at the seam: the objective is the user's own `/goal` text, so an
        // unescaped closing tag in it would break out of this element and forge
        // top-level prompt sections. Single source: `xml_util`.
        output.push_str(&crate::thinker::xml_util::escape_xml(goal));
        output.push_str("\n</standing_goal>\n\n");
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

    fn ctx_with_goal(goal: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
        );
        ctx.standing_goal = goal.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        StandingGoalLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn no_goal_emits_nothing() {
        let out = render(&ctx_with_goal(None));
        assert!(out.is_empty());
    }

    #[test]
    fn goal_renders_inside_tag() {
        let goal = "Ship the standing-goal feature end-to-end (status=active)";
        let out = render(&ctx_with_goal(Some(goal)));
        assert!(out.starts_with("<standing_goal>\n"));
        assert!(out.contains("Ship the standing-goal feature"));
        assert!(out.trim_end().ends_with("</standing_goal>"));
    }

    #[test]
    fn empty_goal_emits_nothing() {
        // The `goal.is_empty()` guard: a present-but-empty summary must still
        // leave the prompt byte-identical.
        let out = render(&ctx_with_goal(Some("")));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_context_emits_nothing() {
        // No resolved context at all (the `input.context` None early-return).
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        StandingGoalLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn name_and_priority() {
        assert_eq!(StandingGoalLayer.name(), "standing_goal");
        assert_eq!(StandingGoalLayer.priority(), 1755);
    }
}
