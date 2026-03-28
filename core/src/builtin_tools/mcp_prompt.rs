//! MCP Prompt Tool
//!
//! Allows LLM to get prompts from connected MCP servers.

use std::collections::HashMap;
use std::pin::Pin;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::tools::AlephToolDyn;

/// Arguments for mcp_get_prompt tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpGetPromptArgs {
    /// Prompt name (e.g., "server_name:prompt_name")
    pub name: String,
    /// Optional arguments to pass to the prompt
    #[serde(default)]
    pub arguments: Option<HashMap<String, Value>>,
}

/// Message in prompt output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptOutputMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
}

/// Output from mcp_get_prompt tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpGetPromptOutput {
    /// Optional description
    pub description: Option<String>,
    /// Prompt messages
    pub messages: Vec<PromptOutputMessage>,
}

/// Tool for getting MCP prompts
pub struct McpGetPromptTool {
    handle: McpManagerHandle,
}

impl McpGetPromptTool {
    /// Create a new MCP get prompt tool
    pub fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AlephToolDyn for McpGetPromptTool {
    fn name(&self) -> &str {
        "mcp_get_prompt"
    }

    fn definition(&self) -> ToolDefinition {
        let schema = schemars::schema_for!(McpGetPromptArgs);
        let parameters: Value = serde_json::to_value(&schema).unwrap_or_default();
        ToolDefinition::new(
            "mcp_get_prompt",
            "Get a prompt template from a connected MCP server. Use mcp.listPrompts to discover available prompts first.",
            parameters,
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpGetPromptArgs = serde_json::from_value(args)?;

            // Extract server_id from prompt name prefix (e.g., "server_name:prompt_name")
            // Try each colon position to handle server IDs with colons (e.g., "plugin:foo/bar")
            let name = &args.name;
            let (client, prompt_name) = {
                let mut found = None;
                for (idx, _) in name.match_indices(':') {
                    let candidate = &name[..idx];
                    if let Ok(Some(c)) = self.handle.get_client(candidate).await {
                        found = Some((c, &name[idx + 1..]));
                        break;
                    }
                }
                match found {
                    Some(pair) => pair,
                    None => {
                        return Err(crate::error::AlephError::NotFound(
                            format!("No MCP server found for prompt: {}", name)
                        ).into());
                    }
                }
            };

            let result = client.get_prompt(prompt_name, args.arguments).await?;

            let messages: Vec<PromptOutputMessage> = result
                .messages
                .into_iter()
                .map(|m| {
                    let content = match m.content {
                        crate::mcp::PromptContent::Text { text } => text,
                        crate::mcp::PromptContent::Image { data, mime_type } => {
                            // data is base64-encoded; approximate decoded size
                            let approx_bytes = data.len() * 3 / 4;
                            format!("[Image: {} (~{} bytes)]", mime_type, approx_bytes)
                        }
                        crate::mcp::PromptContent::Resource { uri, text } => {
                            text.unwrap_or_else(|| format!("[Resource: {}]", uri))
                        }
                    };
                    PromptOutputMessage {
                        role: m.role,
                        content,
                    }
                })
                .collect();

            let output = McpGetPromptOutput {
                description: result.description,
                messages,
            };

            Ok(serde_json::to_value(output)?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_args_schema() {
        let schema = schemars::schema_for!(McpGetPromptArgs);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("name"));
        assert!(json.contains("arguments"));
    }

    #[test]
    fn test_args_deserialize() {
        let json = json!({
            "name": "server:code_review",
            "arguments": {"code": "fn main() {}"}
        });
        let args: McpGetPromptArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.name, "server:code_review");
        assert!(args.arguments.is_some());
    }

    #[test]
    fn test_args_without_arguments() {
        let json = json!({"name": "server:simple_prompt"});
        let args: McpGetPromptArgs = serde_json::from_value(json).unwrap();
        assert!(args.arguments.is_none());
    }
}
