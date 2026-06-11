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
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Finishing & Escalation\n");
        // Universal verification gate (openclaw completion_contract parity):
        // "run the smallest meaningful gate before declaring done", with a
        // concrete menu, provider-agnostic. The conceptual "verify before
        // finalizing" line stays OpenAI-only in provider_guidance.rs.
        output.push_str(
            "- Before declaring the task done, run the smallest meaningful verification gate \
             that fits the work (test/build/lint, re-read the file you wrote, diff your change, \
             or sample the output); if no gate can run, say why.\n",
        );
        output.push_str(
            "- Your final reply IS the deliverable. Report what was accomplished, key \
             results/findings (data, metrics), files produced and their purpose, and caveats — \
             never a bare \"Task completed\".\n",
        );
        output.push_str(
            "- `ask_user`: call it when clarification or a user decision genuinely changes \
             what you'd do next.\n",
        );
        // Goal-level give-up discipline. The method-vs-goal distinction
        // itself lives once in TOOL_PERSISTENCE_DOCTRINE (priority 810);
        // here we add the preconditions for conceding the GOAL.
        output.push_str(
            "- Declare failure ONLY when the GOAL itself cannot be completed in this \
             environment — never on first-method failure. Before conceding: try at least TWO \
             distinct approaches (different tools, sources, or keywords — for web research, \
             climb the fallback ladder above), and enumerate every method tried and why each \
             failed (status code, error class, empty result) so the user can verify nothing \
             was skipped. If the iteration budget is the constraint — not feasibility — \
             deliver the partial result plus the list of unfinished items instead.\n\n",
        );

        // Phase 3 self-evolution path α — direct user-correction signaling.
        // The flag_user_correction tool persists a tagged raw_memory row that
        // FeedbackDistill later distills into a feedback/ knowledge note.
        output.push_str("## Self-correction Logging\n\n");
        output.push_str(
            "When the user corrects you, states a clear preference, or pushes back, call \
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
             own reasoning. Continue normally, and do not announce that you logged it.\n\n",
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
        // Universal verification gate: the model runs the smallest meaningful
        // check (test/build/lint/diff/re-read) before declaring done.
        assert!(out.contains("smallest meaningful verification gate"));
        assert!(out.contains("Task completed"));
        // `ask_user` is a real registered tool and may be named as a call
        // target; `complete` / `fail` are NOT tools and must not be taught
        // as callable actions (legacy {reasoning, action} envelope residue).
        assert!(out.contains("`ask_user`"));
        assert!(!out.contains("- `complete`"));
        assert!(!out.contains("- `fail`"));
        // Give-up discipline: goal-level failure only, alternative-method
        // exhaustion + enumeration required. A single 401 from one source
        // must NOT be enough to concede.
        assert!(out.contains("ONLY when the GOAL itself cannot be completed"));
        assert!(out.contains("at least TWO distinct approaches"));
        assert!(out.contains("fallback ladder"));
        // Budget-bound runs deliver partials instead of conceding.
        assert!(out.contains("partial result"));
    }

    #[test]
    fn test_special_actions_priority() {
        assert_eq!(SpecialActionsLayer.priority(), 1100);
    }

    #[test]
    fn system_prompt_contains_self_correction_logging() {
        // Phase 3 Task 20: prompt must instruct the model to call
        // flag_user_correction conservatively and silently.
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
            out.contains("do not announce"),
            "prompt must instruct silent logging"
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
