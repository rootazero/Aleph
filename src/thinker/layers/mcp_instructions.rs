//! `McpInstructionsLayer` — MCP server instruction injection (priority 1705)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct McpInstructionsLayer;

impl PromptLayer for McpInstructionsLayer {
    fn name(&self) -> &'static str {
        "mcp_instructions"
    }
    fn priority(&self) -> u32 {
        1705
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let instructions = match input.mcp_instructions {
            Some(items) if !items.is_empty() => items,
            _ => return,
        };

        // Filter out entries with empty instructions
        let non_empty: Vec<_> = instructions
            .iter()
            .filter(|i| !i.instructions.is_empty())
            .collect();

        if non_empty.is_empty() {
            return;
        }

        output.push_str("## MCP Server Instructions\n\n");
        output.push_str(
            "The following MCP servers have provided instructions \
             for how to use their tools and resources:\n\n",
        );

        for item in &non_empty {
            output.push_str("### ");
            output.push_str(&item.server_name);
            output.push('\n');
            // Server-supplied free text crossing a trust boundary: sanitized,
            // AND length-capped. This layer only passes the string through and
            // sits in `prompt_contract::CONDITIONALLY_SILENT`, so the per-layer
            // byte ratchet measures it as 0 B — an MCP server advertising a
            // 50 KB instruction block would otherwise land whole in the
            // per-turn dynamic tail. The bound lives here because the layer
            // builds its own text (no separate producer module).
            let sanitized = sanitize_for_prompt(&item.instructions, SanitizeLevel::Light);
            output.push_str(&crate::utils::text_format::truncate_reserving(
                &sanitized,
                MCP_INSTRUCTIONS_MAX_CHARS,
                "\n[instructions truncated]",
            ));
            output.push_str("\n\n");
        }
    }
}

/// Per-server ceiling on rendered MCP instructions — see `inject` above.
const MCP_INSTRUCTIONS_MAX_CHARS: usize = 2_000;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::McpServerInstruction;

    fn make_instruction(name: &str, instructions: &str) -> McpServerInstruction {
        McpServerInstruction {
            server_name: name.to_string(),
            instructions: instructions.to_string(),
        }
    }

    #[test]
    fn injects_for_connected_servers() {
        let layer = McpInstructionsLayer;
        let config = PromptConfig::default();
        let instructions = vec![
            make_instruction("github", "Use GitHub tools for repo management."),
            make_instruction("slack", "Use Slack tools for messaging."),
        ];
        let input = LayerInput::basic(&config, &[]).with_mcp_instructions(&instructions);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## MCP Server Instructions"));
        assert!(out.contains("### github"));
        assert!(out.contains("Use GitHub tools for repo management."));
        assert!(out.contains("### slack"));
        assert!(out.contains("Use Slack tools for messaging."));
    }

    #[test]
    fn skips_empty_instructions() {
        let layer = McpInstructionsLayer;
        let config = PromptConfig::default();
        let instructions = vec![
            make_instruction("github", "Use GitHub tools."),
            make_instruction("empty_server", ""),
        ];
        let input = LayerInput::basic(&config, &[]).with_mcp_instructions(&instructions);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("### github"));
        assert!(!out.contains("### empty_server"));
    }

    #[test]
    fn skips_when_none() {
        let layer = McpInstructionsLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn stability_is_dynamic() {
        assert_eq!(McpInstructionsLayer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn priority_is_1705() {
        assert_eq!(McpInstructionsLayer.priority(), 1705);
    }
}
