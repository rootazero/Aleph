//! ToolsLayer + HydratedToolsLayer — tool discovery and injection (priority 500/501)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

// ---------------------------------------------------------------------------
// ToolsLayer — Basic, Soul, Context, Cached paths
// ---------------------------------------------------------------------------

pub struct ToolsLayer;

impl PromptLayer for ToolsLayer {
    fn name(&self) -> &'static str { "tools" }
    fn priority(&self) -> u32 { 500 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Context,
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
                            output.push_str(&format!("Parameters: {}\n", schema_str));
                        }
                    }
                    output.push('\n');
                }
            }

            if let Some(ref index) = input.config.tool_index {
                output.push_str("### Additional Tools (use `get_tool_schema` to get parameters)\n");
                output.push_str("The following tools are available but not shown with full parameters.\n");
                output.push_str(
                    "Call `get_tool_schema(tool_name)` to get the complete parameter schema before using.\n\n",
                );
                output.push_str(index);
                output.push('\n');
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HydratedToolsLayer — Hydration path only (priority 501 to avoid ambiguity
// with ToolsLayer, even though they are mutually exclusive by path)
// ---------------------------------------------------------------------------

pub struct HydratedToolsLayer;

impl PromptLayer for HydratedToolsLayer {
    fn name(&self) -> &'static str { "hydrated_tools" }
    fn priority(&self) -> u32 { 501 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Hydration]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let result = match input.hydration {
            Some(h) => h,
            None => return,
        };

        if result.is_empty() {
            output.push_str("## Available Tools\n");
            output.push_str("No semantically relevant tools found. Use `get_tool_schema` to discover tools.\n\n");
            return;
        }

        output.push_str("## Available Tools\n\n");

        // Full schema tools - highest relevance, include complete parameter info
        if !result.full_schema_tools.is_empty() {
            output.push_str("### Tools (full parameters)\n\n");
            for tool in &result.full_schema_tools {
                output.push_str(&format!("#### {}\n", tool.name));
                output.push_str(&format!("{}\n", tool.description));
                if let Some(schema) = tool.schema_json() {
                    output.push_str(&format!("Parameters: {}\n", schema));
                }
                output.push('\n');
            }
        }

        // Summary tools - medium relevance, description only
        if !result.summary_tools.is_empty() {
            output.push_str("### Tools (summary - call `get_tool_schema` for parameters)\n\n");
            for tool in &result.summary_tools {
                output.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
            }
            output.push('\n');
        }

        // Indexed tools - low relevance, just names
        if !result.indexed_tool_names.is_empty() {
            output.push_str("### Additional Tools (call `get_tool_schema` to use)\n\n");
            output.push_str(&result.indexed_tool_names.join(", "));
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::ToolInfo;
    use crate::dispatcher::tool_index::HydrationResult;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_tools_with_entries() {
        let layer = ToolsLayer;
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "bash".to_string(),
            description: "Run shell commands".to_string(),
            parameters_schema: Some(serde_json::json!({"command": "string"})),
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
            tool_index: Some("- web_search: Search the web\n- screenshot: Take screenshot".to_string()),
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
    fn test_hydrated_tools_empty() {
        let layer = HydratedToolsLayer;
        let config = PromptConfig::default();
        let hydration = HydrationResult::empty();
        let input = LayerInput::hydration(&config, &hydration);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("No semantically relevant tools found"));
    }

    #[test]
    fn test_hydrated_tools_no_hydration() {
        let layer = HydratedToolsLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools); // no hydration
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
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
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Output should be empty when native tools enabled
        assert!(out.is_empty(), "ToolsLayer should skip when native_tools_enabled=true");
    }
}
