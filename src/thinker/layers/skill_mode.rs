//! SkillModeLayer — strict skill execution mode (priority 1400)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct SkillModeLayer;

impl PromptLayer for SkillModeLayer {
    fn name(&self) -> &'static str {
        "skill_mode"
    }
    fn priority(&self) -> u32 {
        1400
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
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if input.config.skill_mode {
            output.push_str("## Skill Execution Mode — CRITICAL RULES\n\n");
            output.push_str("You are executing a SKILL workflow. Obey exactly:\n\n");
            output.push_str("### RESPONSE FORMAT (MANDATORY)\n");
            output.push_str(
                "Every response MUST be a JSON action object — never raw content. Shape: \
                 `{\"reasoning\": \"...\", \"action\": {...}}`. To save processed data, write it \
                 with the `file_ops` write action; don't emit it inline.\n\n",
            );
            output.push_str("### Workflow\n");
            output.push_str("1. Complete every step — skip none, even if it looks redundant.\n");
            output.push_str("2. Generate every specified output file via `file_ops` write.\n");
            output.push_str("3. Before `complete`, verify all required outputs exist.\n\n");
            output.push_str(
                "If you output raw content instead of a JSON action, you have FAILED.\n\n",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skill_mode_active() {
        let layer = SkillModeLayer;
        let config = PromptConfig {
            skill_mode: true,
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Skill Execution Mode"));
        assert!(out.contains("CRITICAL RULES"));
        assert!(out.contains("RESPONSE FORMAT (MANDATORY)"));
        assert!(out.contains("you have FAILED"));
    }

    #[test]
    fn test_skill_mode_inactive() {
        let layer = SkillModeLayer;
        let config = PromptConfig::default(); // skill_mode = false
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
