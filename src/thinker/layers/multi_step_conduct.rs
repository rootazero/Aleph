//! `MultiStepConductLayer` — teaches the model to plan multi-step work and to
//! narrate progress in interactive conversations (priority 805).
//!
//! Closes two prompt gaps:
//!   1. Plan *creation* needs a stated trigger, not just a mechanism.
//!      `ExecutionPlanLayer` only re-surfaces a plan that already exists; and
//!      the first version of this section phrased the trigger as the model's
//!      judgment call ("for genuinely multi-step work") — weaker models
//!      reliably judge "just write the code" and never open a plan (observed
//!      on MiniMax-M3, 2026-08-27). The trigger is now a rule: 3+ steps, or
//!      any build/fix/create of code or files, starts with `scratchpad`
//!      BEFORE the first file or shell call.
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
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
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

    fn stability(&self) -> LayerStability {
        LayerStability::Stable
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

        // Section 1 — plan first, then execute. The scratchpad mechanism is
        // Aleph-specific and cannot be inferred, so name the tool AND the
        // sequence. The trigger is stated as a rule, not a suggestion: the
        // earlier soft phrasing ("for genuinely multi-step work") left the
        // decision to the model, and weaker models reliably answered it with
        // "just write the code" — observed twice in a row on MiniMax-M3
        // (2026-08-27, two snake-game sessions that never opened a plan).
        // "A single file_write finishes it" is the standard rationalization,
        // so the build/fix/create case is named explicitly.
        output.push_str("## Planning Multi-Step Work\n\n");
        output.push_str(
            "Tasks taking 3+ steps, and any build or fix of code or files, start with \
             `scratchpad` BEFORE any file or shell call: `set_objective`, then `set_plan` \
             (ordered list), then work it (`start_item` / `complete_item`). Never write code \
             before the plan exists; it is the task list the user sees.\n\n",
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

    /// Renamed from `stability_is_stable_by_default`: there is no default any
    /// more. This layer's copy is a constant, so it belongs in the cacheable
    /// prefix — and it now says so itself instead of inheriting the answer by
    /// staying silent. (The old name was the only reason a `grep "fn stability"`
    /// audit believed this file already declared one.)
    #[test]
    fn stability_is_declared_stable() {
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

    /// Section 1 states the plan-first rule and names the mechanism.
    #[test]
    fn interactive_paradigm_emits_both_sections() {
        let out = render(&ctx_for(InteractionParadigm::WebRich));
        assert!(out.contains("## Planning Multi-Step Work"));
        assert!(out.contains("scratchpad"));
        assert!(out.contains("set_objective") && out.contains("set_plan"));
        assert!(out.contains("start_item") && out.contains("complete_item"));
        // The rule, not a suggestion: plan BEFORE the first mutating call.
        assert!(out.contains("BEFORE any file or shell call"));
        assert!(out.contains("Never write code before the plan exists"));
        // The single-step carve-out lives in the tool's own DESCRIPTION (one
        // statement, one place), so this layer does not repeat it.
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
