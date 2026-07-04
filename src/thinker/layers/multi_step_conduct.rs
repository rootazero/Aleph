//! `MultiStepConductLayer` — teaches the model to plan multi-step work and to
//! narrate progress in interactive conversations (priority 805).
//!
//! Closes two prompt gaps:
//!   1. Nothing told the model *when* to autonomously reach for the
//!      `scratchpad` tool. `ExecutionPlanLayer` only re-surfaces a plan that
//!      already exists; it never triggers plan *creation*. So a task list only
//!      appeared when the user hand-typed a trigger phrase.
//!   2. Across a long run of tool calls the model emitted no visible text, so
//!      the interactive panel showed only a "thinking" spinner. The streaming
//!      pipeline already forwards every assistant delta live — the model just
//!      was never told to speak between steps.
//!
//! Both fixes are pure prompt guidance (R7/R9): the harness makes no
//! completion judgment and runs no extra LLM call. R10-safe — this lives in
//! `src/thinker/layers/`, not `src/harness/`.
//!
//! Gating mirrors `ProtocolTokensLayer`'s inverse: the whole layer is withheld
//! whenever the `SilentReply` capability is active (Background / cron), where
//! silent completion is the point and the prompt must stay byte-identical.

use crate::thinker::interaction::Capability;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MultiStepConductLayer;

impl PromptLayer for MultiStepConductLayer {
    fn name(&self) -> &'static str {
        "multi_step_conduct"
    }

    fn priority(&self) -> u32 {
        805
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        // Ride every non-minimal path; the `inject()` guard keeps output empty
        // when no `ResolvedContext` is attached or SilentReply is active.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };

        // Single gate: interactive paradigms only. When SilentReply is active
        // (Background / cron) emit nothing so those prompts stay byte-identical
        // and `ALEPH_SILENT_COMPLETE` (taught by ProtocolTokensLayer) is the
        // only protocol in play there.
        if ctx
            .environment_contract
            .active_capabilities
            .contains(&Capability::SilentReply)
        {
            return;
        }

        // Section 1 — when to plan.
        output.push_str("## Planning Multi-Step Work\n\n");
        output.push_str(
            "When a request genuinely needs several ordered steps, spans multiple phases, or \
             asks for more than one distinct thing, plan before you act: use the `scratchpad` \
             tool to set an objective and lay out an execution list, then work it one item at a \
             time with `start_item` / `complete_item`.\n\n",
        );
        output.push_str(
            "Do not plan trivial work. A direct answer, a single tool call, or anything that \
             finishes in one or two steps needs no scratchpad — just do it. Decide from the shape \
             of the task; don't wait to be told. Stay flexible: drop the plan if the task turns \
             out simpler than expected, or start one mid-task if it grows.\n\n",
        );

        // Section 2 — narrate progress (interactive only, same gate as above).
        output.push_str("## Narrate Your Progress\n\n");
        output.push_str(
            "This is an interactive conversation and the user is watching. Don't work silently \
             across many tool calls. In your visible reply (not hidden thinking):\n",
        );
        output.push_str(
            "- Before an action or a batch of related actions, post a one-line preamble \
             (roughly 8-12 words) of what you're about to do. Group logically related \
             actions under ONE preamble — don't narrate every single tool call.\n",
        );
        output.push_str(
            "- Skip the preamble for a single trivial read (opening one file, one quick \
             lookup); narrate the batch it belongs to instead.\n",
        );
        output.push_str(
            "- Connect each preamble to what came before — e.g. \"Config found — now \
             wiring the new field.\" — so progress reads as one thread.\n",
        );
        output.push_str(
            "- After finishing each plan step, post a brief recap, e.g. \"Done: the data \
             model is in place.\", so the user can follow along.\n\n",
        );
        output.push_str(
            "Keep these to a sentence or two — enough to show momentum, not so much that it \
             clutters the conversation.\n\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::{LayerInput, LayerStability};
    use crate::thinker::security_context::SecurityContext;

    fn ctx_for(paradigm: InteractionParadigm) -> ResolvedContext {
        ContextAggregator::resolve(
            &InteractionManifest::new(paradigm),
            &SecurityContext::permissive(),
            &[],
        )
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        MultiStepConductLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn name_matches_module() {
        assert_eq!(MultiStepConductLayer.name(), "multi_step_conduct");
    }

    #[test]
    fn priority_is_805() {
        assert_eq!(MultiStepConductLayer.priority(), 805);
    }

    #[test]
    fn stability_is_stable_by_default() {
        assert!(matches!(
            MultiStepConductLayer.stability(),
            LayerStability::Stable
        ));
    }

    #[test]
    fn excluded_from_minimal_mode() {
        assert!(!MultiStepConductLayer.supports_mode(PromptMode::Minimal));
        assert!(MultiStepConductLayer.supports_mode(PromptMode::Full));
    }

    #[test]
    fn no_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        MultiStepConductLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn interactive_paradigm_emits_both_sections() {
        let out = render(&ctx_for(InteractionParadigm::WebRich));
        assert!(out.contains("## Planning Multi-Step Work"));
        assert!(out.contains("scratchpad"));
        // Lock the anti-over-trigger copy so a future edit can't silently drop it.
        assert!(out.contains("Do not plan trivial work"));
        assert!(out.contains("## Narrate Your Progress"));
        assert!(out.contains("Group logically related actions"));
        assert!(out.contains("Skip the preamble for a single trivial read"));
    }

    #[test]
    fn silent_paradigm_emits_nothing() {
        // Background carries SilentReply → whole layer is withheld so the
        // background prompt stays byte-identical.
        let out = render(&ctx_for(InteractionParadigm::Background));
        assert!(out.is_empty());
    }
}
