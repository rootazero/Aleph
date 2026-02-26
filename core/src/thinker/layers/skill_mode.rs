//! SkillModeLayer — strict skill execution mode (priority 1400)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SkillModeLayer;

impl PromptLayer for SkillModeLayer {
    fn name(&self) -> &'static str { "skill_mode" }
    fn priority(&self) -> u32 { 1400 }
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
            output.push_str("## ⚠️ Skill Execution Mode - CRITICAL RULES\n\n");
            output.push_str("You are executing a SKILL workflow. You MUST follow these rules EXACTLY:\n\n");
            output.push_str("### 🔴 RESPONSE FORMAT (MANDATORY)\n");
            output.push_str("**EVERY response MUST be a valid JSON action object. NEVER output raw content directly!**\n\n");
            output.push_str("❌ WRONG: Outputting processed text, data, or results directly\n");
            output.push_str("✅ CORRECT: Always return {\"reasoning\": \"...\", \"action\": {...}}\n\n");
            output.push_str("If you need to process data and save it, use the `file_ops` tool:\n");
            output.push_str("```json\n");
            output.push_str("{\"reasoning\": \"Writing processed data to file\", \"action\": {\"type\": \"tool\", \"tool_name\": \"file_ops\", \"arguments\": {\"operation\": \"write\", \"path\": \"output.json\", \"content\": \"...\"}}}\n");
            output.push_str("```\n\n");
            output.push_str("### Workflow Requirements\n");
            output.push_str("1. Complete ALL steps in the skill workflow - NO exceptions\n");
            output.push_str("2. Generate ALL output files specified (JSON, .mmd, .txt, images, etc.)\n");
            output.push_str("3. Use `file_ops` with `operation: \"write\"` to save each file\n");
            output.push_str("4. DO NOT skip any step, even if you think it's redundant\n");
            output.push_str("5. Before calling `complete`, verify ALL required outputs exist\n\n");
            output.push_str("### Common skill outputs to generate\n");
            output.push_str("- Data files: `triples.json`, `*.json`\n");
            output.push_str("- Visualization code: `graph.mmd`, `*.mmd`\n");
            output.push_str("- Prompts: `image-prompt.txt`, `*.txt`\n");
            output.push_str("- Images: via `generate_image` tool\n");
            output.push_str("- Merged outputs: `merged-*.json`, `full-*.mmd`\n\n");
            output.push_str("**If you output raw content instead of JSON action, you have FAILED.**\n\n");
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
