//! ResponseFormatLayer — JSON response format specification (priority 1200)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ResponseFormatLayer;

impl PromptLayer for ResponseFormatLayer {
    fn name(&self) -> &'static str { "response_format" }
    fn priority(&self) -> u32 { 1200 }
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
        output.push_str("## Response Format\n");
        output.push_str("You must respond with a JSON object:\n");
        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"reasoning\": \"Brief explanation of your thinking\",\n");
        output.push_str("  \"action\": {\n");
        output.push_str("    \"type\": \"tool|ask_user|complete|fail\",\n");
        output.push_str("    \"tool_name\": \"...\",      // if type=tool\n");
        output.push_str("    \"arguments\": {...},       // if type=tool\n");
        output.push_str("    \"question\": \"...\",        // if type=ask_user\n");
        output.push_str("    \"options\": [...],         // if type=ask_user (optional)\n");
        output.push_str("    \"summary\": \"...\",         // if type=complete (MUST be detailed report)\n");
        output.push_str("    \"reason\": \"...\"           // if type=fail\n");
        output.push_str("  }\n");
        output.push_str("}\n");
        output.push_str("```\n\n");
        output.push_str("### ask_user Format Details\n");
        output.push_str("When using `ask_user`, you have TWO modes:\n\n");

        output.push_str("**Mode 1: Single Question** (simple selection or text input)\n");
        output.push_str("- Use `options` field as an array of SEPARATE choices:\n");
        output.push_str("  - ✅ CORRECT: [\"Option 1\", \"Option 2\", \"Option 3\"]\n");
        output.push_str("  - ❌ WRONG: [\"Option 1 / Option 2 / Option 3\"] (single merged string)\n");
        output.push_str("- Each option should be a standalone, selectable choice\n");
        output.push_str("- If no options (free-form text input), omit the field or use empty array\n\n");

        output.push_str("**Mode 2: Multi-Group Questions** (multiple related questions)\n");
        output.push_str("Use this when you need answers to MULTIPLE independent questions simultaneously.\n");
        output.push_str("Instead of asking one by one, group them together for better UX.\n\n");

        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"reasoning\": \"Need multiple image generation parameters\",\n");
        output.push_str("  \"action\": {\n");
        output.push_str("    \"type\": \"ask_user_multigroup\",\n");
        output.push_str("    \"question\": \"Please configure the image generation settings\",  // Overall prompt\n");
        output.push_str("    \"groups\": [\n");
        output.push_str("      {\n");
        output.push_str("        \"id\": \"format\",  // Unique group ID (alphanumeric)\n");
        output.push_str("        \"prompt\": \"Output format\",\n");
        output.push_str("        \"options\": [\"PNG\", \"JPEG\", \"WebP\"]\n");
        output.push_str("      },\n");
        output.push_str("      {\n");
        output.push_str("        \"id\": \"quality\",\n");
        output.push_str("        \"prompt\": \"Quality level\",\n");
        output.push_str("        \"options\": [\"Low\", \"Medium\", \"High\", \"Best\"]\n");
        output.push_str("      },\n");
        output.push_str("      {\n");
        output.push_str("        \"id\": \"size\",\n");
        output.push_str("        \"prompt\": \"Image size\",\n");
        output.push_str("        \"options\": [\"512x512\", \"1024x1024\", \"2048x2048\"]\n");
        output.push_str("      }\n");
        output.push_str("    ]\n");
        output.push_str("  }\n");
        output.push_str("}\n");
        output.push_str("```\n\n");

        output.push_str("**When to use Multi-Group**:\n");
        output.push_str("- Multiple configuration options needed (3+ choices)\n");
        output.push_str("- Questions are independent but related\n");
        output.push_str("- Better UX than asking one-by-one\n");
        output.push_str("- Example: \"Choose format (PNG/JPEG), quality (Low/Medium/High), size (Small/Large)\"\n\n");

        output.push_str("**When NOT to use Multi-Group**:\n");
        output.push_str("- Single question with multiple options → Use simple `ask_user`\n");
        output.push_str("- Questions depend on previous answers → Ask sequentially\n");
        output.push_str("- Free-form text input → Use `ask_user` with no options\n\n");

        output.push_str("**Simple ask_user Example**:\n");
        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"reasoning\": \"Need user to select image format\",\n");
        output.push_str("  \"action\": {\n");
        output.push_str("    \"type\": \"ask_user\",\n");
        output.push_str("    \"question\": \"Which output format do you prefer?\",\n");
        output.push_str("    \"options\": [\"PNG\", \"JPEG\", \"WebP\"]\n");
        output.push_str("  }\n");
        output.push_str("}\n");
        output.push_str("```\n\n");
        output.push_str("### Completion Summary Format\n");
        output.push_str("When `type=complete`, the `summary` should be a well-formatted report:\n");
        output.push_str("```\n");
        output.push_str("## Task Completed\n");
        output.push_str("[Brief description of what was accomplished]\n\n");
        output.push_str("### Results\n");
        output.push_str("[Key findings, data, or outcomes]\n\n");
        output.push_str("### Generated Files\n");
        output.push_str("- file1.json: [description]\n");
        output.push_str("- file2.png: [description]\n\n");
        output.push_str("### Notes\n");
        output.push_str("[Any recommendations or important observations]\n");
        output.push_str("```\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_response_format_content() {
        let layer = ResponseFormatLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Response Format"));
        assert!(out.contains("\"reasoning\""));
        assert!(out.contains("ask_user_multigroup"));
        assert!(out.contains("Completion Summary Format"));
    }

    #[test]
    fn test_response_format_priority() {
        assert_eq!(ResponseFormatLayer.priority(), 1200);
    }
}
