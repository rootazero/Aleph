//! SpecialActionsLayer — special action definitions (priority 1100)

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
        output.push_str("## Special Actions\n");
        // `complete` carries a universal verification gate (pass-3 addition):
        // openclaw's completion_contract makes "run the smallest meaningful
        // gate before declaring done" apply to every model, with a concrete
        // menu (test/build/lint/diff/re-read). Aleph previously had only the
        // conceptual "verify before finalizing" line, and only in the
        // OpenAI-family tail (provider_guidance.rs). Putting the actionable
        // gate on the `complete` action makes it provider-agnostic without
        // duplicating that conceptual line.
        output.push_str("- `complete`: Call when the task is fully done — but FIRST run the smallest meaningful verification gate that fits the work (run the test/build/lint, re-read the file you wrote, diff your change, or sample the output); if no gate can run, say why in the summary. The `summary` field MUST report: overview of what was accomplished, key results/findings (data, metrics), generated files and their purpose, notes/recommendations. **DO NOT** just say 'Task completed' — write a summary the user can act on directly.\n");
        output.push_str("- `ask_user`: Call when you need clarification or a user decision.\n");
        output.push_str(
            "- `fail`: Call ONLY when the GOAL itself cannot be completed in this environment \
             — never on first-method failure. Preconditions:\n",
        );
        output.push_str(
            "  1. Tried at least TWO distinct approaches (different tools, sources, or \
             keywords) — for web research, climb the fallback ladder above.\n",
        );
        output.push_str(
            "  2. `summary` enumerates every method tried and why each failed (status code, \
             error class, empty result) so the user can verify nothing was skipped.\n",
        );
        output.push_str(
            "  3. Distinguish *method failure* (\"Reuters returned 401\" — a routing signal; \
             switch and continue) from *goal failure* (\"every source I can reach is offline\"). \
             Only the latter justifies `fail`.\n",
        );
        output.push_str(
            "  4. If iteration budget is the constraint (not feasibility), prefer `complete` \
             with a partial result + list of unfinished items — not `fail`.\n\n",
        );

        // Phase 3 self-evolution path α — direct user-correction signaling.
        // The flag_user_correction tool persists a tagged raw_memory row that
        // FeedbackDistill later distills into a feedback/ knowledge note.
        output.push_str("## Self-correction Logging\n\n");
        output.push_str(
            "When the user corrects you, states a clear preference, or pushes back, call \
             `flag_user_correction` to record the signal. Provide:\n",
        );
        output.push_str("- `content`: the correction in your own words (1-2 sentences)\n");
        output.push_str(
            "- `severity`: low (one-off) / med (project rule) / high (strong directive) / \
             critical (absolute redline)\n",
        );
        output.push_str("- `suggested_rule` (optional): a one-line imperative for next time\n\n");
        output.push_str(
            "Use it conservatively — only clear, generalizable signals. Skip praise, \
             acknowledgement, and your own reasoning. Continue normally, and do not announce \
             that you logged it.\n\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_special_actions_content() {
        let layer = SpecialActionsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Special Actions"));
        assert!(out.contains("`complete`"));
        assert!(out.contains("`ask_user`"));
        assert!(out.contains("`fail`"));
        assert!(out.contains("DO NOT"));
        // Pass-3: `complete` must carry the universal verification gate so the
        // model runs the smallest meaningful check (test/build/lint/diff/
        // re-read) before declaring done — provider-agnostic, not OpenAI-only.
        assert!(out.contains("smallest meaningful verification gate"));
        // `fail` must require alternative-method exhaustion + enumeration in
        // the summary. A single 401 from one source must NOT be enough to
        // justify the `fail` action.
        assert!(out.contains("ONLY when the GOAL itself cannot be completed"));
        assert!(out.contains("at least TWO distinct approaches"));
        assert!(out.contains("fallback ladder"));
        assert!(out.contains("method failure"));
        assert!(out.contains("goal failure"));
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
            out.contains("conservatively"),
            "prompt must instruct conservative use"
        );
        assert!(
            out.contains("do not announce"),
            "prompt must instruct silent logging"
        );
        // Section ordering — Self-correction must come AFTER Special Actions
        // so the model is already grounded in the tool catalogue when it
        // reads about the meta-correction signal.
        let special_pos = out
            .find("## Special Actions")
            .expect("special actions header present");
        let correction_pos = out
            .find("## Self-correction Logging")
            .expect("self-correction header present");
        assert!(special_pos < correction_pos);
    }
}
