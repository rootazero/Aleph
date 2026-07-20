//! `ToolsLayer` — tool discovery and injection (priority 500)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

// ---------------------------------------------------------------------------
// ToolsLayer — Basic, Soul, Context, Cached paths
// ---------------------------------------------------------------------------

pub struct ToolsLayer;

impl PromptLayer for ToolsLayer {
    fn name(&self) -> &'static str {
        "tools"
    }
    fn priority(&self) -> u32 {
        500
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if input.config.native_tools_enabled {
            return; // Tools passed via API native tool_use, not system prompt
        }

        // Context path: use available_tools from ResolvedContext
        let tools = if let Some(ctx) = input.context {
            &ctx.available_tools[..]
        } else {
            match input.tools {
                Some(t) => t,
                None => &[],
            }
        };

        output.push_str("## Available Tools\n");
        if tools.is_empty() && input.config.tool_index.is_none() {
            output.push_str("No tools available. You can only use special actions.\n\n");
        } else {
            if !tools.is_empty() {
                output.push_str("### Tools (with full parameters)\n");
                for tool in tools {
                    output.push_str(&format!("#### {}\n", tool.name));
                    output.push_str(&format!("{}\n", tool.description));
                    if let Some(ref schema) = tool.parameters_schema {
                        let schema_str = serde_json::to_string(schema).unwrap_or_default();
                        if !schema_str.is_empty() {
                            output.push_str(&format!("Parameters: {schema_str}\n"));
                        }
                    }
                    output.push('\n');
                }
            }

            if let Some(ref index) = input.config.tool_index {
                output.push_str("### Additional Tools (use `get_tool_schema` to get parameters)\n");
                output.push_str(
                    "Available but not shown with full parameters. Use `search_tools(query)` to find one, then `get_tool_schema(tool_name)` for its schema before use.\n\n",
                );
                output.push_str(index);
                output.push('\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::tools::info::ToolInfo;

    #[test]
    fn test_tools_with_entries() {
        let layer = ToolsLayer;
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "bash".to_string(),
            description: "Run shell commands".to_string(),
            parameters_schema: Some(serde_json::json!({"command": "string"})),
            usage_hint: None,
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Available Tools"));
        assert!(out.contains("#### bash"));
        assert!(out.contains("Run shell commands"));
        assert!(out.contains("Parameters:"));
    }

    #[test]
    fn test_tools_empty() {
        let layer = ToolsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("No tools available"));
    }

    #[test]
    fn test_tools_with_index() {
        let layer = ToolsLayer;
        let config = PromptConfig {
            tool_index: Some(
                "- web_search: Search the web\n- screenshot: Take screenshot".to_string(),
            ),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Additional Tools"));
        assert!(out.contains("get_tool_schema"));
        assert!(out.contains("web_search"));
    }

    #[test]
    fn test_tools_skipped_when_native_tools_enabled() {
        let layer = ToolsLayer;
        let config = PromptConfig {
            native_tools_enabled: true,
            ..Default::default()
        };
        let tools = vec![ToolInfo {
            name: "bash".to_string(),
            description: "Run shell commands".to_string(),
            parameters_schema: Some(serde_json::json!({"command": "string"})),
            usage_hint: None,
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Output should be empty when native tools enabled
        assert!(
            out.is_empty(),
            "ToolsLayer should skip when native_tools_enabled=true"
        );
    }
}
