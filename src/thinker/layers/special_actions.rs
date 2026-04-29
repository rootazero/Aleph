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
        output.push_str("- `complete`: Call when the task is fully done. The `summary` field MUST be a comprehensive report that includes:\n");
        output.push_str("  1. A brief overview of what was accomplished\n");
        output.push_str("  2. Key results and findings (data, insights, metrics)\n");
        output.push_str("  3. List of all generated files with their purposes\n");
        output.push_str("  4. Any important notes or recommendations\n");
        output.push_str(
            "  **DO NOT** just say 'Task completed'. Write a detailed summary the user can immediately understand.\n",
        );
        output.push_str("- `ask_user`: Call when you need clarification or user decision\n");
        output.push_str("- `fail`: Call when the task cannot be completed\n\n");

        // Phase 3 self-evolution path α — direct user-correction signaling.
        // The flag_user_correction tool persists a tagged raw_memory row that
        // FeedbackDistill later distills into a feedback/ knowledge note.
        output.push_str("## Self-correction Logging\n\n");
        output.push_str(
            "When the user corrects you, expresses a clear preference, or pushes back on \
             your approach, call the `flag_user_correction` tool to record the signal so \
             the system can learn from it. Provide:\n",
        );
        output.push_str("- `content`: the user's correction in your own words (1-2 sentences)\n");
        output.push_str(
            "- `severity`: low (one-off preference) / med (project-level rule) / high \
             (strong directive) / critical (absolute redline)\n",
        );
        output.push_str(
            "- `suggested_rule` (optional): a one-line imperative for how you should behave \
             next time\n\n",
        );
        output.push_str(
            "Use this proactively but conservatively — only when the signal is clear and \
             generalizable. Do NOT flag praise, neutral acknowledgement, or your own \
             internal reasoning. Continue the conversation normally after flagging; \
             do not announce that you logged the correction.\n\n",
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
