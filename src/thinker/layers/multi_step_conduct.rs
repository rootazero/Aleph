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
        &[AssemblyPath::Basic, AssemblyPath::Cached]
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

        // Section 1 — the scratchpad planning tool exists (Aleph-specific
        // mechanism the model can't infer). The old when-to-plan / don't-plan-
        // trivial cognition prose was cut — a capable model decides that from
        // the task shape (§1.1 prune-the-prompt).
        output.push_str("## Planning Multi-Step Work\n\n");
        output.push_str(
            "For genuinely multi-step work, use the `scratchpad` tool to set an objective and an \
             execution list, then work it one item at a time with `start_item` / `complete_item`. \
             Skip it for anything that finishes in a step or two.\n\n",
        );

        // Section 2 — narrate progress. The non-inferable fact is the UX one:
        // tool calls stream to the user live, so silence reads as a stalled
        // spinner. The old preamble word-counts and sample phrasings ("Config
        // found — now wiring…") were a few-shot cage and were cut.
        output.push_str("## Narrate Your Progress\n\n");
        output.push_str(
            "This is an interactive conversation and your tool calls stream to the user live, so \
             don't work in silence across many steps. Post a short line of intent before a batch \
             of related actions and a brief recap after, in your visible reply — a sentence or \
             two, enough to show momentum without clutter.\n\n",
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
        assert!(out.contains("start_item") && out.contains("complete_item"));
        // Anti-over-trigger intent preserved in compressed form.
        assert!(out.contains("Skip it for anything that finishes in a step or two"));
        assert!(out.contains("## Narrate Your Progress"));
        assert!(out.contains("stream to the user live"));
        // The sample-phrasing cage and narration micromanagement are gone.
        assert!(!out.contains("Config found"));
        assert!(!out.contains("8-12 words"));
    }

    #[test]
    fn silent_paradigm_emits_nothing() {
        // Background carries SilentReply → whole layer is withheld so the
        // background prompt stays byte-identical.
        let out = render(&ctx_for(InteractionParadigm::Background));
        assert!(out.is_empty());
    }
}
