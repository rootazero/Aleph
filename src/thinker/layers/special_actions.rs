//! `SpecialActionsLayer` — finishing / escalation discipline (priority 1100)
//!
//! Historical note: this layer used to document `complete` / `fail` as
//! callable actions of the legacy `{reasoning, action}` JSON envelope.
//! That envelope (and its `ResponseFormatLayer`) was removed when the
//! harness moved to native `with_tools(...)` — `complete` and `fail`
//! never existed in the builtin tool registry, so teaching them as
//! call targets made the prompt instruct the model to invoke
//! nonexistent tools. The *behaviours* they encoded (verification gate
//! before declaring done, actionable summaries, goal-level give-up
//! discipline) are preserved below as plain conduct rules; `ask_user`
//! and `flag_user_correction` remain real registered tools.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct SpecialActionsLayer;

impl PromptLayer for SpecialActionsLayer {
    fn name(&self) -> &'static str {
        "special_actions"
    }
    fn priority(&self) -> u32 {
        1100
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Finishing & Escalation\n");
        // Only the non-inferable deliverable-quality rule + the `ask_user`
        // pointer stay. The verification-gate menu and the goal-level give-up /
        // fallback-ladder discipline were cut as how-to a capable model runs
        // natively (§1.1 prune-the-prompt); they duplicated
        // `ProviderGuidanceLayer`'s persistence doctrine, now the single home.
        output.push_str(
            "- Your final reply IS the deliverable. Report what was accomplished, key \
             results/findings (data, metrics), files produced and their purpose, and caveats — \
             never a bare \"Task completed\".\n",
        );
        output.push_str(
            "- `ask_user`: call it when clarification or a user decision genuinely changes \
             what you'd do next.\n\n",
        );

        // A `## Self-correction Logging` block used to live here, spelling out
        // when to call `flag_user_correction`, its three arguments, the
        // low/med/high/critical severity ladder, and the one-sentence
        // acknowledgment contract. Every one of those sentences is already in
        // `FlagUserCorrectionTool::DESCRIPTION` and the `FlagUserCorrectionArgs`
        // field docs — several verbatim — and the tool schema is delivered with
        // every request that can call it. Restating it here bought nothing and
        // created a second copy that could drift from the first (pi's rule:
        // tool semantics live with the tool; the system prompt carries only what
        // no single tool can state). Cross-tool routing — which memory
        // destination wins — is the thing no tool can state, and it stays in
        // `memory_protocol.rs`'s ladder.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_finishing_discipline_content() {
        let layer = SpecialActionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Finishing & Escalation"));
        assert!(out.contains("Task completed"));
        // `ask_user` is a real registered tool and may be named as a call
        // target; `complete` / `fail` are NOT tools and must not be taught
        // as callable actions (legacy {reasoning, action} envelope residue).
        assert!(out.contains("`ask_user`"));
        assert!(!out.contains("- `complete`"));
        assert!(!out.contains("- `fail`"));
        // The verification-gate / give-up / fallback-ladder how-to was cut as
        // a cage a capable model doesn't need (§1.1 prune-the-prompt); it
        // duplicated ProviderGuidanceLayer's persistence doctrine.
        assert!(!out.contains("verification gate"));
        assert!(!out.contains("fallback ladder"));
        assert!(!out.contains("at least TWO distinct approaches"));
    }

    #[test]
    fn test_special_actions_priority() {
        assert_eq!(SpecialActionsLayer.priority(), 1100);
    }

    #[test]
    fn self_correction_how_to_lives_in_the_tool_not_the_prompt() {
        // The D4 acknowledgment contract is not gone — it is (and already was)
        // stated in `FlagUserCorrectionTool::DESCRIPTION`, which ships with the
        // tool schema on every request that can call the tool. This layer must
        // not carry a second copy: two homes for one rule is how they drift.
        let layer = SpecialActionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(!out.contains("## Self-correction Logging"));
        assert!(
            !out.contains("flag_user_correction"),
            "tool how-to belongs to the tool, not the always-on prompt"
        );

        // The surviving single home still states the whole contract.
        let desc =
            <crate::builtin_tools::FlagUserCorrectionTool as crate::tools::AlephTool>::DESCRIPTION;
        assert!(desc.contains("corrects a mistake you made"));
        assert!(desc.contains("ONE") && desc.contains("user's language"));
        assert!(desc.contains("destination"));
        assert!(desc.contains("Never quote the stored content back verbatim"));
        assert!(desc.contains("never log the same correction"));
    }
}
