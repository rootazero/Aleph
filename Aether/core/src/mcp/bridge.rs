//! MCP Tool Bridge
//!
//! Bridges MCP tools (both builtin and external) to the AgentTool interface.
//! This allows MCP tools to be used seamlessly with the native function calling
//! infrastructure.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     McpToolBridge                                │
//! │  (Implements AgentTool trait for MCP tools)                     │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  tool_def: McpTool      │  Metadata from MCP server             │
//! │  client: Arc<McpClient> │  Reference for tool execution         │
//! │  source: McpToolSource  │  Tool origin (builtin/external)       │
//! └─────────────────────────────────────────────────────────────────┘
//!                                   │
//!                                   ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                       McpClient                                  │
//! │  call_tool(name, args) → McpToolResult                          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::mcp::{McpClient, McpToolBridge, McpToolSource};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create MCP client
//! let client = Arc::new(McpClient::new());
//!
//! // Create bridges for all MCP tools
//! let bridges = McpToolBridge::from_client(client.clone()).await;
//!
//! // Register in native tool registry
//! let registry = NativeToolRegistry::new();
//! for bridge in bridges {
//!     registry.register(Arc::new(bridge)).await;
//! }
//! ```

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::{AetherError, Result};
use crate::mcp::client::McpClient;
use crate::mcp::types::McpTool;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Tool source - where the MCP tool originates from
#[derive(Debug, Clone)]
pub enum McpToolSource {
    /// Builtin service (Tier 1 - native Rust)
    Builtin {
        /// Service name (e.g., "fs", "git", "shell")
        service_name: String,
    },
    /// External MCP server (Tier 2 - external process)
    External {
        /// Server name
        server_name: String,
    },
}

impl McpToolSource {
    /// Get the source identifier
    pub fn identifier(&self) -> &str {
        match self {
            McpToolSource::Builtin { service_name } => service_name,
            McpToolSource::External { server_name } => server_name,
        }
    }

    /// Check if this is a builtin source
    pub fn is_builtin(&self) -> bool {
        matches!(self, McpToolSource::Builtin { .. })
    }
}

/// MCP Tool Bridge
///
/// Bridges an MCP tool (either builtin or external) to the AgentTool interface.
/// This allows MCP tools to be used with the native function calling system.
pub struct McpToolBridge {
    /// Tool definition from MCP
    tool_def: McpTool,
    /// MCP client for tool execution
    client: Arc<McpClient>,
    /// Tool source (builtin or external)
    source: McpToolSource,
}

impl McpToolBridge {
    /// Create a new MCP tool bridge
    ///
    /// # Arguments
    ///
    /// * `tool_def` - The MCP tool definition
    /// * `client` - Arc reference to the MCP client
    /// * `source` - The tool source (builtin/external)
    pub fn new(tool_def: McpTool, client: Arc<McpClient>, source: McpToolSource) -> Self {
        Self {
            tool_def,
            client,
            source,
        }
    }

    /// Create bridges for all tools from an MCP client
    ///
    /// Creates `McpToolBridge` instances for all registered external MCP server tools.
    ///
    /// Note: Native tools (fs, git, shell, etc.) are now handled via the `AgentTool`
    /// infrastructure in the `tools` module, not via MCP bridges.
    pub async fn from_client(client: Arc<McpClient>) -> Vec<Self> {
        let mut bridges = Vec::new();

        // Bridge external server tools only
        // Native tools are now handled via AgentTool infrastructure
        let all_tools = client.list_tools().await;
        for tool in all_tools {
            // External tools have format "server_name:tool_name"
            let server_name = tool
                .name
                .split(':')
                .next()
                .unwrap_or("unknown")
                .to_string();

            bridges.push(Self::new(
                tool,
                Arc::clone(&client),
                McpToolSource::External { server_name },
            ));
        }

        bridges
    }

    /// Create bridges for builtin tools only (sync version)
    ///
    /// Note: Native tools are now handled via the `AgentTool` infrastructure
    /// in the `tools` module. McpClient no longer stores builtin services.
    /// This method returns an empty vector for backward compatibility.
    #[deprecated(note = "Native tools are now handled via AgentTool. Use tools module instead.")]
    pub fn from_client_builtin_only(_client: Arc<McpClient>) -> Vec<Self> {
        // No builtin tools in McpClient anymore
        // Native tools are handled via AgentTool infrastructure
        Vec::new()
    }

    /// Get the tool source
    pub fn source(&self) -> &McpToolSource {
        &self.source
    }

    /// Get the service/server name
    pub fn source_name(&self) -> &str {
        self.source.identifier()
    }

    /// Convert McpTool to ToolDefinition
    fn to_tool_definition(&self) -> ToolDefinition {
        let category = self.infer_category();

        ToolDefinition {
            name: self.tool_def.name.clone(),
            description: self.tool_def.description.clone(),
            parameters: self.tool_def.input_schema.clone(),
            requires_confirmation: self.tool_def.requires_confirmation,
            category,
        }
    }

    /// Infer tool category from source and tool name
    fn infer_category(&self) -> ToolCategory {
        match &self.source {
            McpToolSource::Builtin { service_name } => {
                match service_name.as_str() {
                    "fs" => ToolCategory::Filesystem,
                    "git" => ToolCategory::Git,
                    "shell" => ToolCategory::Shell,
                    "sys" | "system" => ToolCategory::System,
                    "clipboard" => ToolCategory::Clipboard,
                    "screen" => ToolCategory::Screen,
                    "search" => ToolCategory::Search,
                    _ => ToolCategory::Other,
                }
            }
            McpToolSource::External { .. } => ToolCategory::External,
        }
    }

    /// Convert McpToolResult to ToolResult
    fn convert_result(mcp_result: crate::mcp::types::McpToolResult) -> ToolResult {
        if mcp_result.success {
            // Extract content as string
            let content = match &mcp_result.content {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };

            // If content is an object/array, include as data
            if mcp_result.content.is_object() || mcp_result.content.is_array() {
                ToolResult::success_with_data(content.clone(), mcp_result.content)
            } else {
                ToolResult::success(content)
            }
        } else {
            ToolResult::error(mcp_result.error.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }
}

#[async_trait]
impl AgentTool for McpToolBridge {
    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn definition(&self) -> ToolDefinition {
        self.to_tool_definition()
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse JSON arguments
        let args_value: serde_json::Value = serde_json::from_str(args).map_err(|e| {
            AetherError::InvalidConfig {
                message: format!("Invalid JSON arguments for tool '{}': {}", self.tool_def.name, e),
                suggestion: Some("Ensure arguments are valid JSON".to_string()),
            }
        })?;

        // Call through MCP client
        let mcp_result = self
            .client
            .call_tool(&self.tool_def.name, args_value)
            .await?;

        Ok(Self::convert_result(mcp_result))
    }

    fn requires_confirmation(&self) -> bool {
        self.tool_def.requires_confirmation
    }

    fn category(&self) -> ToolCategory {
        self.infer_category()
    }
}

/// Helper to create bridges from a client without consuming it
pub async fn create_bridges(client: &Arc<McpClient>) -> Vec<Arc<dyn AgentTool>> {
    McpToolBridge::from_client(Arc::clone(client))
        .await
        .into_iter()
        .map(|b| Arc::new(b) as Arc<dyn AgentTool>)
        .collect()
}

/// Helper to create builtin-only bridges (sync)
///
/// Note: Native tools are now handled via the `AgentTool` infrastructure
/// in the `tools` module. This function returns an empty vector.
#[deprecated(note = "Native tools are now handled via AgentTool. Use tools module instead.")]
pub fn create_builtin_bridges(_client: &Arc<McpClient>) -> Vec<Arc<dyn AgentTool>> {
    // No builtin tools in McpClient anymore
    // Native tools are handled via AgentTool infrastructure
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::types::McpToolResult;
    use serde_json::json;

    fn create_test_tool() -> McpTool {
        McpTool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            requires_confirmation: false,
        }
    }

    #[test]
    fn test_mcp_tool_source_builtin() {
        let source = McpToolSource::Builtin {
            service_name: "fs".to_string(),
        };

        assert!(source.is_builtin());
        assert_eq!(source.identifier(), "fs");
    }

    #[test]
    fn test_mcp_tool_source_external() {
        let source = McpToolSource::External {
            server_name: "my-server".to_string(),
        };

        assert!(!source.is_builtin());
        assert_eq!(source.identifier(), "my-server");
    }

    #[test]
    fn test_bridge_to_definition() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::Builtin {
                service_name: "fs".to_string(),
            },
        );

        let def = bridge.definition();
        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(!def.requires_confirmation);
        assert_eq!(def.category, ToolCategory::Filesystem);
    }

    #[test]
    fn test_bridge_category_inference_fs() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::Builtin {
                service_name: "fs".to_string(),
            },
        );

        assert_eq!(bridge.category(), ToolCategory::Filesystem);
    }

    #[test]
    fn test_bridge_category_inference_git() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::Builtin {
                service_name: "git".to_string(),
            },
        );

        assert_eq!(bridge.category(), ToolCategory::Git);
    }

    #[test]
    fn test_bridge_category_inference_external() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::External {
                server_name: "some-server".to_string(),
            },
        );

        assert_eq!(bridge.category(), ToolCategory::External);
    }

    #[test]
    fn test_convert_result_success_string() {
        let mcp_result = McpToolResult::success(json!("Hello, world!"));
        let result = McpToolBridge::convert_result(mcp_result);

        assert!(result.success);
        assert_eq!(result.content, "Hello, world!");
        assert!(result.data.is_none());
    }

    #[test]
    fn test_convert_result_success_object() {
        let mcp_result = McpToolResult::success(json!({"status": "ok", "count": 42}));
        let result = McpToolBridge::convert_result(mcp_result);

        assert!(result.success);
        assert!(result.data.is_some());

        let data = result.data.unwrap();
        assert_eq!(data["status"], "ok");
        assert_eq!(data["count"], 42);
    }

    #[test]
    fn test_convert_result_error() {
        let mcp_result = McpToolResult::error("Something went wrong");
        let result = McpToolBridge::convert_result(mcp_result);

        assert!(!result.success);
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_bridge_name() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::Builtin {
                service_name: "test".to_string(),
            },
        );

        assert_eq!(bridge.name(), "test_tool");
    }

    #[test]
    fn test_bridge_requires_confirmation_false() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::Builtin {
                service_name: "test".to_string(),
            },
        );

        assert!(!bridge.requires_confirmation());
    }

    #[test]
    fn test_bridge_requires_confirmation_true() {
        let mut tool = create_test_tool();
        tool.requires_confirmation = true;

        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(
            tool,
            client,
            McpToolSource::Builtin {
                service_name: "shell".to_string(),
            },
        );

        assert!(bridge.requires_confirmation());
    }

    #[test]
    #[allow(deprecated)]
    fn test_from_client_builtin_only_empty() {
        let client = Arc::new(McpClient::new());
        let bridges = McpToolBridge::from_client_builtin_only(client);

        // Empty client has no builtin tools (deprecated - native tools now via AgentTool)
        assert!(bridges.is_empty());
    }

    #[tokio::test]
    async fn test_from_client_empty() {
        let client = Arc::new(McpClient::new());
        let bridges = McpToolBridge::from_client(client).await;

        // Empty client has no tools
        assert!(bridges.is_empty());
    }

    #[tokio::test]
    async fn test_create_bridges_empty() {
        let client = Arc::new(McpClient::new());
        let bridges = create_bridges(&client).await;

        assert!(bridges.is_empty());
    }

    #[test]
    #[allow(deprecated)]
    fn test_create_builtin_bridges_empty() {
        let client = Arc::new(McpClient::new());
        let bridges = create_builtin_bridges(&client);

        // Deprecated - native tools now via AgentTool
        assert!(bridges.is_empty());
    }
}
