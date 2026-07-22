//! `ExecutionPlanLayer` — emits `<execution_plan>` at priority 1756 (Dynamic).
//!
//! Re-surfaces the session's active scratchpad execution list (objective +
//! checklist + current step) into the system prompt **every turn** while
//! work remains. This closes the one gap Aleph's scratchpad had vs the
//! reference agents: codex `update_plan` keeps the plan persistently visible
//! in its UI, opencode `todowrite` and Claude Code's `TodoWrite` re-inject the
//! list as a per-turn reminder, and hermes-agent's `/goal` loop re-states the
//! goal in every continuation. Aleph previously only echoed progress inside
//! the `scratchpad` tool's own result (i.e. only on turns the model called
//! it) plus a stop-time verifier — so across a long run of bash/edit/read
//! tool calls the plan dropped out of context and the model could lose the
//! thread.
//!
//! R10-safe: pure scaffolding. The content is the model's *own* checklist
//! (the boxes it ticked), rendered verbatim — the harness makes no
//! completion judgment, runs no extra LLM call, and applies no relevance
//! scoring. `Dynamic` stability keeps it out of the cached stable prefix;
//! `None` (no active plan with pending work) emits nothing, leaving the
//! prompt byte-identical for sessions that never touched the scratchpad.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ExecutionPlanLayer;

impl PromptLayer for ExecutionPlanLayer {
    fn name(&self) -> &'static str {
        "execution_plan"
    }

    fn priority(&self) -> u32 {
        1756
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        // Mirror the dynamic-zone neighbours (SessionResume / SessionContext
        // Guide). The data lives in `ResolvedContext`; paths that carry no
        // context hit the `None` early-return and emit nothing. The live
        // cached prompt executes the `Cached` path.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }

    fn stability(&self) -> LayerStability {
        // Plan progress changes per turn — keep it in the dynamic suffix so
        // it never invalidates the cached stable prefix.
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // The plan is operational steering, not chrome — drop it only from
        // the bare-bones Minimal prompt.
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(plan) = ctx.execution_plan.as_deref() else {
            return;
        };
        if plan.is_empty() {
            return;
        }
        output.push_str("<execution_plan>\n");
        output.push_str(plan);
        output.push_str(
            "\n\nThis is your standing plan for the current objective. Keep working \
             the list one step at a time; update it with the `scratchpad` tool \
             (start_item / complete_item) as you make progress.\n",
        );
        output.push_str("</execution_plan>\n\n");
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

    fn ctx_with_plan(plan: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.execution_plan = plan.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        ExecutionPlanLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn priority_is_1756() {
        assert_eq!(ExecutionPlanLayer.priority(), 1756);
    }

    #[test]
    fn name_matches_module() {
        assert_eq!(ExecutionPlanLayer.name(), "execution_plan");
    }

    #[test]
    fn stability_is_dynamic() {
        assert!(matches!(
            ExecutionPlanLayer.stability(),
            LayerStability::Dynamic
        ));
    }

    #[test]
    fn excluded_from_minimal_mode() {
        assert!(!ExecutionPlanLayer.supports_mode(PromptMode::Minimal));
        assert!(ExecutionPlanLayer.supports_mode(PromptMode::Full));
    }

    #[test]
    fn no_plan_emits_nothing() {
        let out = render(&ctx_with_plan(None));
        assert!(out.is_empty());
    }

    #[test]
    fn empty_plan_emits_nothing() {
        let out = render(&ctx_with_plan(Some("")));
        assert!(out.is_empty());
    }

    #[test]
    fn missing_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        ExecutionPlanLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn plan_renders_inside_tag() {
        let plan = "Objective: ship feature\nPlan:\n- [x] design\n- [ ] build\nProgress: 1/2 done";
        let out = render(&ctx_with_plan(Some(plan)));
        assert!(out.starts_with("<execution_plan>\n"));
        assert!(out.contains("Objective: ship feature"));
        assert!(out.contains("- [ ] build"));
        assert!(out.contains("scratchpad"));
        assert!(out.trim_end().ends_with("</execution_plan>"));
    }
}
