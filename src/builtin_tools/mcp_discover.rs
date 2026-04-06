//! McpToolSchemaTool — return full MCP tool schema for on-demand discovery.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::mcp::McpClient;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for mcp_tool_schema
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpToolSchemaArgs {
    /// Full tool name (e.g., "github:create_issue")
    pub tool_name: String,
}

/// Output from mcp_tool_schema containing full tool definition
#[derive(Debug, Clone, Serialize)]
pub struct McpToolSchemaOutput {
    /// Full tool name
    pub tool_name: String,
    /// Server name extracted from tool name prefix
    pub server_name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
    /// Whether this tool requires user confirmation
    pub requires_confirmation: bool,
}

impl fmt::Display for McpToolSchemaOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Tool: {} (server: {})", self.tool_name, self.server_name)?;
        writeln!(f, "  description: {}", self.description)?;
        writeln!(
            f,
            "  schema: {}",
            serde_json::to_string_pretty(&self.input_schema).unwrap_or_default()
        )?;
        writeln!(f, "  requires_confirmation: {}", self.requires_confirmation)?;
        Ok(())
    }
}

/// Tool for retrieving full MCP tool schemas on demand.
///
/// Allows the LLM to discover the full parameter schema for any MCP
/// server tool so it can construct correct tool calls.
#[derive(Clone)]
pub struct McpToolSchemaTool {
    mcp_client: Arc<McpClient>,
}

impl McpToolSchemaTool {
    /// Create a new McpToolSchemaTool with a shared MCP client reference.
    pub fn new(mcp_client: Arc<McpClient>) -> Self {
        Self { mcp_client }
    }
}

#[async_trait]
impl AlephTool for McpToolSchemaTool {
    const NAME: &'static str = "mcp_tool_schema";
    const DESCRIPTION: &'static str =
        "Get the full parameter schema for an MCP server tool. \
         Returns the tool's JSON Schema input definition so you can call it correctly.";

    type Args = McpToolSchemaArgs;
    type Output = McpToolSchemaOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"mcp_tool_schema({"tool_name": "github:create_issue"})"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(tool_name = %args.tool_name, "mcp_tool_schema requested");

        let tools = self.mcp_client.list_tools().await;
        let tool = tools
            .iter()
            .find(|t| t.name == args.tool_name)
            .ok_or_else(|| {
                let available: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
                AlephError::McpToolNotFound(format!(
                    "MCP tool '{}' not found. Available: {}",
                    args.tool_name,
                    available.join(", ")
                ))
            })?;

        let server_name = args
            .tool_name
            .split(':')
            .next()
            .unwrap_or(&args.tool_name)
            .to_string();

        Ok(McpToolSchemaOutput {
            tool_name: tool.name.clone(),
            server_name,
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            requires_confirmation: tool.requires_confirmation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_serialization() {
        let output = McpToolSchemaOutput {
            tool_name: "github:create_issue".to_string(),
            server_name: "github".to_string(),
            description: "Create an issue".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"}
                }
            }),
            requires_confirmation: false,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("github:create_issue"));
        assert!(json.contains("Create an issue"));
    }

    #[test]
    fn test_server_name_extraction() {
        let name = "github:create_issue";
        let server = name.split(':').next().unwrap_or(name);
        assert_eq!(server, "github");

        let name2 = "standalone_tool";
        let server2 = name2.split(':').next().unwrap_or(name2);
        assert_eq!(server2, "standalone_tool");
    }

    #[test]
    fn test_output_display() {
        let output = McpToolSchemaOutput {
            tool_name: "slack:send".to_string(),
            server_name: "slack".to_string(),
            description: "Send a message".to_string(),
            input_schema: serde_json::json!({}),
            requires_confirmation: true,
        };
        let display = format!("{}", output);
        assert!(display.contains("slack:send"));
        assert!(display.contains("requires_confirmation: true"));
    }
}
