//! Builtin Tool Registry for Agent Loop
//!
//! This module provides a `BuiltinToolRegistry` that implements the `ToolRegistry` trait,
//! allowing the Agent Loop's SingleStepExecutor to directly invoke builtin tools without
//! going through rig's agent framework.
//!
//! # Safety Features
//!
//! The registry integrates with the Three-Layer Control architecture's CapabilityGate
//! to enforce capability-based access control on tool execution.
//!
//! # Usage
//!
//! ```ignore
//! use aethecore::executor::{BuiltinToolRegistry, SingleStepExecutor};
//! use aethecore::three_layer::{Capability, CapabilityGate};
//!
//! // Create registry with capability restrictions
//! let gate = CapabilityGate::new(vec![
//!     Capability::FileRead,
//!     Capability::WebSearch,
//! ]);
//! let registry = BuiltinToolRegistry::with_gate(gate);
//! let executor = SingleStepExecutor::new(Arc::new(registry));
//! ```

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::error::{AetherError, Result};
use crate::generation::GenerationProviderRegistry;
use crate::rig_tools::{FileOpsTool, SearchTool, WebFetchTool, YouTubeTool};
use crate::three_layer::{Capability, CapabilityGate};

use super::ToolRegistry;

/// Configuration for builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Generation provider registry for image/video/audio generation
    pub generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
}

/// Registry of builtin tools for Agent Loop
///
/// Holds instances of builtin tools and provides direct invocation capabilities.
/// Integrates with CapabilityGate for security enforcement.
pub struct BuiltinToolRegistry {
    /// Search tool instance
    search_tool: SearchTool,
    /// Web fetch tool instance
    web_fetch_tool: WebFetchTool,
    /// YouTube tool instance
    youtube_tool: YouTubeTool,
    /// File operations tool instance
    file_ops_tool: FileOpsTool,
    /// Tool metadata for lookup
    tools: HashMap<String, UnifiedTool>,
    /// Capability gate for security enforcement
    capability_gate: CapabilityGate,
}

impl BuiltinToolRegistry {
    /// Create a new registry with default configuration
    ///
    /// Uses a permissive CapabilityGate that allows safe operations by default.
    pub fn new() -> Self {
        Self::with_config(BuiltinToolConfig::default())
    }

    /// Create a new registry with custom configuration
    ///
    /// Uses a permissive CapabilityGate that allows safe operations by default.
    pub fn with_config(config: BuiltinToolConfig) -> Self {
        // Default: allow safe operations (read, search, fetch)
        let gate = CapabilityGate::new(vec![
            Capability::FileRead,
            Capability::FileList,
            Capability::WebSearch,
            Capability::WebFetch,
            Capability::LlmCall,
        ]);
        Self::with_config_and_gate(config, gate)
    }

    /// Create a new registry with custom capability gate
    ///
    /// Allows fine-grained control over which operations are permitted.
    pub fn with_gate(gate: CapabilityGate) -> Self {
        Self::with_config_and_gate(BuiltinToolConfig::default(), gate)
    }

    /// Create a new registry with custom configuration and capability gate
    pub fn with_config_and_gate(config: BuiltinToolConfig, capability_gate: CapabilityGate) -> Self {
        let search_tool = SearchTool::with_api_key(config.tavily_api_key);
        let web_fetch_tool = WebFetchTool::new();
        let youtube_tool = YouTubeTool::new();
        let file_ops_tool = FileOpsTool::new();

        // Build tool metadata
        let mut tools = HashMap::new();

        tools.insert(
            "search".to_string(),
            UnifiedTool::new(
                "builtin:search",
                "search",
                SearchTool::DESCRIPTION,
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "web_fetch".to_string(),
            UnifiedTool::new(
                "builtin:web_fetch",
                "web_fetch",
                "Fetch and read content from a URL",
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "youtube".to_string(),
            UnifiedTool::new(
                "builtin:youtube",
                "youtube",
                "Get information about YouTube videos",
                ToolSource::Builtin,
            ),
        );

        tools.insert(
            "file_ops".to_string(),
            UnifiedTool::new(
                "builtin:file_ops",
                "file_ops",
                "File system operations - list, read, write, move, copy, delete, etc.",
                ToolSource::Builtin,
            ),
        );

        // TODO: Add image generation tool when generation_registry is provided

        Self {
            search_tool,
            web_fetch_tool,
            youtube_tool,
            file_ops_tool,
            tools,
            capability_gate,
        }
    }

    /// Get the required capability for a tool
    fn required_capability(&self, tool_name: &str, arguments: &Value) -> Option<Capability> {
        match tool_name {
            "search" => Some(Capability::WebSearch),
            "web_fetch" => Some(Capability::WebFetch),
            "youtube" => Some(Capability::WebFetch), // YouTube fetches from web
            "file_ops" => {
                // Determine capability based on operation type
                if let Some(op) = arguments.get("operation").and_then(|v| v.as_str()) {
                    match op {
                        "list" | "search" => Some(Capability::FileList),
                        "read" => Some(Capability::FileRead),
                        "write" | "move" | "copy" | "mkdir" | "organize" | "batch_move" => {
                            Some(Capability::FileWrite)
                        }
                        "delete" => Some(Capability::FileDelete),
                        _ => Some(Capability::FileRead), // Default to read for unknown ops
                    }
                } else {
                    Some(Capability::FileRead)
                }
            }
            _ => None,
        }
    }

    /// Check if an operation is permitted by the capability gate
    fn check_capability(&self, tool_name: &str, arguments: &Value) -> Result<()> {
        if let Some(required) = self.required_capability(tool_name, arguments) {
            self.capability_gate.check(&required).map_err(|denied| {
                info!(
                    tool = tool_name,
                    capability = %denied.required,
                    "Capability check failed"
                );
                AetherError::permission_denied(format!(
                    "Operation requires '{}' capability which is not granted",
                    denied.required
                ))
            })
        } else {
            Ok(())
        }
    }

    /// Execute the search tool
    async fn execute_search(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::SearchArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid search arguments: {}", e))
            })?;

        let result = self.search_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Search failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the web fetch tool
    async fn execute_web_fetch(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::WebFetchArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid web_fetch arguments: {}", e))
            })?;

        let result = self.web_fetch_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("Web fetch failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the YouTube tool
    async fn execute_youtube(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::YouTubeArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid youtube arguments: {}", e))
            })?;

        let result = self.youtube_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("YouTube tool failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }

    /// Execute the file operations tool
    async fn execute_file_ops(&self, arguments: Value) -> Result<Value> {
        let args: crate::rig_tools::FileOpsArgs =
            serde_json::from_value(arguments).map_err(|e| {
                AetherError::tool(format!("Invalid file_ops arguments: {}", e))
            })?;

        let result = self.file_ops_tool.call(args).await.map_err(|e| {
            AetherError::tool(format!("File operations failed: {}", e))
        })?;

        serde_json::to_value(result)
            .map_err(|e| AetherError::tool(format!("Failed to serialize result: {}", e)))
    }
}

impl Default for BuiltinToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry for BuiltinToolRegistry {
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool> {
        self.tools.get(name)
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        debug!(tool = tool_name, "Executing builtin tool");

        // Check capability before execution
        if let Err(e) = self.check_capability(tool_name, &arguments) {
            return Box::pin(async move { Err(e) });
        }

        // Match tool name before creating future to avoid lifetime issues
        match tool_name {
            "search" => Box::pin(async move { self.execute_search(arguments).await }),
            "web_fetch" => Box::pin(async move { self.execute_web_fetch(arguments).await }),
            "youtube" => Box::pin(async move { self.execute_youtube(arguments).await }),
            "file_ops" => Box::pin(async move { self.execute_file_ops(arguments).await }),
            _ => {
                let tool = tool_name.to_string();
                error!(tool = %tool, "Unknown tool requested");
                Box::pin(async move {
                    Err(AetherError::tool(format!("Unknown tool: {}", tool)))
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = BuiltinToolRegistry::new();

        // Verify all tools are registered
        assert!(registry.get_tool("search").is_some());
        assert!(registry.get_tool("web_fetch").is_some());
        assert!(registry.get_tool("youtube").is_some());
        assert!(registry.get_tool("file_ops").is_some());

        // Verify unknown tool returns None
        assert!(registry.get_tool("unknown").is_none());
    }

    #[test]
    fn test_tool_metadata() {
        let registry = BuiltinToolRegistry::new();

        let search = registry.get_tool("search").unwrap();
        assert_eq!(search.name, "search");
        assert_eq!(search.id, "builtin:search");
        assert!(matches!(search.source, ToolSource::Builtin));
    }

    #[tokio::test]
    async fn test_unknown_tool_execution() {
        let registry = BuiltinToolRegistry::new();

        let result = registry
            .execute_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown tool"));
    }

    #[test]
    fn test_required_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // Search requires WebSearch
        assert_eq!(
            registry.required_capability("search", &serde_json::json!({})),
            Some(Capability::WebSearch)
        );

        // Web fetch requires WebFetch
        assert_eq!(
            registry.required_capability("web_fetch", &serde_json::json!({})),
            Some(Capability::WebFetch)
        );

        // File ops - read operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "read"})),
            Some(Capability::FileRead)
        );

        // File ops - list operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "list"})),
            Some(Capability::FileList)
        );

        // File ops - write operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "write"})),
            Some(Capability::FileWrite)
        );

        // File ops - delete operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "delete"})),
            Some(Capability::FileDelete)
        );
    }

    #[tokio::test]
    async fn test_capability_check_denied() {
        // Create registry with only WebSearch capability
        let gate = CapabilityGate::new(vec![Capability::WebSearch]);
        let registry = BuiltinToolRegistry::with_gate(gate);

        // Search should work (WebSearch granted)
        let search_result = registry.check_capability("search", &serde_json::json!({}));
        assert!(search_result.is_ok());

        // File ops read should fail (FileRead not granted)
        let file_result =
            registry.check_capability("file_ops", &serde_json::json!({"operation": "read"}));
        assert!(file_result.is_err());
        let err_msg = file_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Permission denied") || err_msg.contains("capability"),
            "Expected permission error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_file_delete_blocked_by_default() {
        // Default registry doesn't grant FileDelete
        let registry = BuiltinToolRegistry::new();

        // Delete should be blocked
        let result = registry
            .execute_tool(
                "file_ops",
                serde_json::json!({"operation": "delete", "path": "/tmp/test"}),
            )
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Permission denied") || err_msg.contains("capability"),
            "Expected permission error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_file_read_allowed_by_default() {
        // Default registry grants FileRead
        let registry = BuiltinToolRegistry::new();

        // Read capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "read"}));
        assert!(check.is_ok());
    }
}
