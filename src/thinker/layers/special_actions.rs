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
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
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

        // Phase 3 self-evolution path α — direct user-correction signaling.
        // The flag_user_correction tool persists a tagged raw_memory row that
        // FeedbackDistill later distills into a feedback/ knowledge note.
        // Destination routing for OTHER memory kinds lives in the Memory
        // Protocol ladder (memory_protocol.rs) — don't duplicate it here.
        output.push_str("## Self-correction Logging\n\n");
        output.push_str(
            "When the user corrects a mistake you made or pushes back on your approach, call \
             `flag_user_correction` with:\n",
        );
        output.push_str("- `content`: the correction in your own words (1-2 sentences)\n");
        output.push_str(
            "- `severity`: low (one-off) / med (project rule) / high (strong directive) / \
             critical (absolute redline)\n",
        );
        output.push_str("- `suggested_rule` (optional): a one-line imperative for next time\n\n");
        output.push_str(
            "Log only clear, generalizable signals — skip praise, acknowledgement, and your \
             own reasoning. Continue normally, then close your reply with ONE short sentence, \
             in the user's language, acknowledging where the lesson was recorded — use the \
             `destination` field from the tool result (e.g. \"Lesson recorded to long-term \
             memory — feedback queue, distilled into a standing note by the nightly cycle.\"). \
             Never quote the stored content back verbatim, never log the same correction \
             twice, and never emit more than one acknowledgment sentence per turn.\n\n",
        );
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
    fn system_prompt_contains_self_correction_logging() {
        // Phase 3 Task 20 + D4 inversion: prompt must instruct the model to
        // call flag_user_correction conservatively, then acknowledge the
        // write in one short sentence (the old silent-logging contract is
        // reversed — the user asked to be told where lessons land).
        let layer = SpecialActionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(
            out.contains("flag_user_correction"),
            "prompt must mention the tool name"
        );
        assert!(
            out.contains("## Self-correction Logging"),
            "prompt must have a clearly delimited section header"
        );
        assert!(
            out.contains("generalizable signals"),
            "prompt must instruct conservative use"
        );
        assert!(
            !out.contains("do not announce"),
            "silent-logging wording must be gone (D4 inversion)"
        );
        assert!(
            out.contains("ONE short sentence") && out.contains("user's language"),
            "prompt must state the one-sentence acknowledgment contract"
        );
        assert!(
            out.contains("Never quote the stored content back verbatim"),
            "acknowledgment must not echo the stored entry"
        );
        assert!(
            out.contains("never log the same correction twice"),
            "prompt must forbid re-logging the same correction"
        );
        // Section ordering — Self-correction must come AFTER the finishing
        // discipline so the model is already grounded in turn-level conduct
        // when it reads about the meta-correction signal.
        let finishing_pos = out
            .find("## Finishing & Escalation")
            .expect("finishing discipline header present");
        let correction_pos = out
            .find("## Self-correction Logging")
            .expect("self-correction header present");
        assert!(finishing_pos < correction_pos);
    }
}
