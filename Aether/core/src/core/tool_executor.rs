//! Unified Tool Executor
//!
//! This module provides a unified execution layer for all tool types:
//! - Builtin capabilities (search, video, memory)
//! - Native tools (AgentTool implementations like web_fetch, file_read)
//! - MCP tools (external MCP server tools)
//!
//! The executor routes tool calls to the appropriate backend based on
//! tool source, enabling intelligent tool invocation from natural language.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::{AetherError, Result};
use crate::mcp::McpClient;
use crate::payload::Capability;
use crate::tools::NativeToolRegistry;

// =============================================================================
// Types
// =============================================================================

/// Source of a tool
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Builtin capability (search, video, memory)
    Builtin,
    /// Native AgentTool implementation
    Native,
    /// MCP server tool
    Mcp,
    /// Skill (future)
    Skill,
}

/// Result from tool execution
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    /// Name of the tool that was executed
    pub tool_name: String,
    /// Whether execution succeeded
    pub success: bool,
    /// Content returned by the tool
    pub content: String,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Source of the tool
    pub source: ToolSource,
}

impl ToolExecutionResult {
    /// Create a successful result
    pub fn success(tool_name: &str, content: String, execution_time_ms: u64, source: ToolSource) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            success: true,
            content,
            error: None,
            execution_time_ms,
            source,
        }
    }

    /// Create a failed result
    pub fn error(tool_name: &str, error: String, execution_time_ms: u64, source: ToolSource) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            success: false,
            content: String::new(),
            error: Some(error),
            execution_time_ms,
            source,
        }
    }
}

/// Information about an available tool
#[derive(Debug, Clone)]
pub struct ToolInfo {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool source
    pub source: ToolSource,
}

// =============================================================================
// UnifiedToolExecutor
// =============================================================================

/// Unified executor that routes tool calls to the appropriate backend
pub struct UnifiedToolExecutor {
    /// Native tool registry
    native_registry: Arc<NativeToolRegistry>,

    /// MCP client for MCP tools
    mcp_client: Option<Arc<McpClient>>,

    /// Cached tool source mapping
    tool_sources: Arc<RwLock<HashMap<String, ToolSource>>>,
}

impl UnifiedToolExecutor {
    /// Create a new UnifiedToolExecutor
    pub fn new(
        native_registry: Arc<NativeToolRegistry>,
        mcp_client: Option<Arc<McpClient>>,
    ) -> Self {
        Self {
            native_registry,
            mcp_client,
            tool_sources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Refresh the tool source cache
    pub async fn refresh_tool_sources(&self) {
        let mut sources = self.tool_sources.write().await;
        sources.clear();

        // Builtin capabilities
        for name in ["search", "video", "youtube", "memory"] {
            sources.insert(name.to_string(), ToolSource::Builtin);
        }

        // Native tools
        for def in self.native_registry.get_definitions().await {
            sources.insert(def.name.clone(), ToolSource::Native);
        }

        // MCP tools
        if let Some(ref client) = self.mcp_client {
            for tool in client.list_tools().await {
                sources.insert(tool.name.clone(), ToolSource::Mcp);
            }
        }

        debug!(
            builtin_count = 4,
            native_count = sources.values().filter(|s| **s == ToolSource::Native).count(),
            mcp_count = sources.values().filter(|s| **s == ToolSource::Mcp).count(),
            "Tool sources refreshed"
        );
    }

    /// Resolve the source of a tool
    pub async fn resolve_source(&self, tool_name: &str) -> Option<ToolSource> {
        let sources = self.tool_sources.read().await;
        sources.get(tool_name).cloned()
    }

    /// Check if a tool exists
    pub async fn has_tool(&self, tool_name: &str) -> bool {
        // Check builtin first
        if matches!(tool_name, "search" | "video" | "youtube" | "memory") {
            return true;
        }

        // Check native registry
        if self.native_registry.contains(tool_name).await {
            return true;
        }

        // Check cached tool sources for MCP
        let sources = self.tool_sources.read().await;
        if sources.get(tool_name) == Some(&ToolSource::Mcp) {
            return true;
        }

        false
    }

    /// Execute a tool by name
    ///
    /// This method routes the execution to the appropriate backend:
    /// - Builtin: Returns None (caller should use existing capability flow)
    /// - Native: Executes via NativeToolRegistry
    /// - MCP: Executes via McpClient
    pub async fn execute(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
    ) -> Result<ToolExecutionResult> {
        let start = Instant::now();

        // Determine tool source (with fallback check)
        let source = self.resolve_source(tool_name).await.unwrap_or_else(|| {
            // If not in cache, try to detect source
            if matches!(tool_name, "search" | "video" | "youtube" | "memory") {
                ToolSource::Builtin
            } else {
                // Default to Native, will fail if not found
                ToolSource::Native
            }
        });

        info!(
            tool = %tool_name,
            source = ?source,
            "Executing tool via UnifiedToolExecutor"
        );

        let result = match source {
            ToolSource::Builtin => {
                // Builtin tools should be handled by the existing capability flow
                // Return a special result indicating caller should use capability executor
                return Err(AetherError::other(format!(
                    "Builtin tool '{}' should be executed via capability flow",
                    tool_name
                )));
            }
            ToolSource::Native => {
                self.execute_native(tool_name, parameters).await
            }
            ToolSource::Mcp => {
                self.execute_mcp(tool_name, parameters).await
            }
            ToolSource::Skill => {
                Err(AetherError::other("Skills not yet implemented"))
            }
        };

        let execution_time_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(content) => {
                info!(
                    tool = %tool_name,
                    source = ?source,
                    execution_time_ms = execution_time_ms,
                    content_length = content.len(),
                    "Tool executed successfully"
                );
                Ok(ToolExecutionResult::success(tool_name, content, execution_time_ms, source))
            }
            Err(e) => {
                warn!(
                    tool = %tool_name,
                    source = ?source,
                    error = %e,
                    "Tool execution failed"
                );
                Ok(ToolExecutionResult::error(tool_name, e.to_string(), execution_time_ms, source))
            }
        }
    }

    /// Execute a native tool
    async fn execute_native(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
    ) -> Result<String> {
        let args = serde_json::to_string(&parameters)?;
        let result = self.native_registry.execute(tool_name, &args).await?;

        if result.is_success() {
            Ok(result.content)
        } else {
            Err(AetherError::other(
                result.error.unwrap_or_else(|| "Tool execution failed".to_string())
            ))
        }
    }

    /// Execute an MCP tool
    async fn execute_mcp(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
    ) -> Result<String> {
        let client = self.mcp_client.as_ref().ok_or_else(|| {
            AetherError::other("MCP client not available")
        })?;

        let result = client.call_tool(tool_name, parameters).await?;

        if result.success {
            serde_json::to_string_pretty(&result.content)
                .map_err(|e| AetherError::other(format!("Failed to serialize MCP result: {}", e)))
        } else {
            Err(AetherError::other(
                result.error.unwrap_or_else(|| "MCP tool failed".to_string())
            ))
        }
    }

    /// Get list of all available tools
    pub async fn list_all_tools(&self) -> Vec<ToolInfo> {
        let mut tools = Vec::new();

        // Builtins
        tools.push(ToolInfo {
            name: "search".to_string(),
            description: "Search the web for information".to_string(),
            source: ToolSource::Builtin,
        });
        tools.push(ToolInfo {
            name: "video".to_string(),
            description: "Get transcript from YouTube videos".to_string(),
            source: ToolSource::Builtin,
        });
        tools.push(ToolInfo {
            name: "memory".to_string(),
            description: "Search conversation memory".to_string(),
            source: ToolSource::Builtin,
        });

        // Native tools
        for def in self.native_registry.get_definitions().await {
            tools.push(ToolInfo {
                name: def.name,
                description: def.description,
                source: ToolSource::Native,
            });
        }

        // MCP tools
        if let Some(ref client) = self.mcp_client {
            for tool in client.list_tools().await {
                tools.push(ToolInfo {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    source: ToolSource::Mcp,
                });
            }
        }

        tools
    }

    /// Map a tool name to Capability (for builtin tools)
    ///
    /// Note: "fetch" and "web_fetch" are native tools, not builtin capabilities.
    /// They are handled by the NativeToolRegistry, not the CapabilityExecutor.
    pub fn resolve_builtin_capability(tool_name: &str) -> Option<Capability> {
        match tool_name {
            "search" => Some(Capability::Search),
            "video" | "youtube" => Some(Capability::Video),
            "memory" => Some(Capability::Memory),
            // "fetch" and "web_fetch" are NOT builtin capabilities
            // They are native tools executed via NativeToolRegistry
            _ => None,
        }
    }

    /// Check if a tool is a builtin capability
    pub fn is_builtin(tool_name: &str) -> bool {
        matches!(tool_name, "search" | "video" | "youtube" | "memory")
    }

    /// Check if a tool name should be handled as a native tool
    ///
    /// These tools are registered in NativeToolRegistry and executed
    /// via UnifiedToolExecutor, not the CapabilityExecutor.
    pub fn is_native_tool(tool_name: &str) -> bool {
        matches!(tool_name, "fetch" | "web_fetch" | "file_read" | "file_write" | "file_list" |
                 "git_status" | "git_diff" | "git_log" | "shell_execute" |
                 "system_info" | "clipboard_read" | "screen_capture")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_executor() -> UnifiedToolExecutor {
        let registry = NativeToolRegistry::new();
        UnifiedToolExecutor::new(Arc::new(registry), None)
    }

    #[test]
    fn test_resolve_builtin_capability() {
        assert_eq!(
            UnifiedToolExecutor::resolve_builtin_capability("search"),
            Some(Capability::Search)
        );
        assert_eq!(
            UnifiedToolExecutor::resolve_builtin_capability("video"),
            Some(Capability::Video)
        );
        assert_eq!(
            UnifiedToolExecutor::resolve_builtin_capability("youtube"),
            Some(Capability::Video)
        );
        assert_eq!(
            UnifiedToolExecutor::resolve_builtin_capability("memory"),
            Some(Capability::Memory)
        );
        assert_eq!(
            UnifiedToolExecutor::resolve_builtin_capability("web_fetch"),
            None
        );
    }

    #[test]
    fn test_is_builtin() {
        assert!(UnifiedToolExecutor::is_builtin("search"));
        assert!(UnifiedToolExecutor::is_builtin("video"));
        assert!(UnifiedToolExecutor::is_builtin("youtube"));
        assert!(UnifiedToolExecutor::is_builtin("memory"));
        assert!(!UnifiedToolExecutor::is_builtin("web_fetch"));
        assert!(!UnifiedToolExecutor::is_builtin("file_read"));
    }

    #[tokio::test]
    async fn test_has_tool_builtin() {
        let executor = create_test_executor();
        assert!(executor.has_tool("search").await);
        assert!(executor.has_tool("video").await);
        assert!(executor.has_tool("memory").await);
    }

    #[tokio::test]
    async fn test_list_all_tools() {
        let executor = create_test_executor();
        let tools = executor.list_all_tools().await;

        // Should have at least builtins
        assert!(tools.iter().any(|t| t.name == "search"));
        assert!(tools.iter().any(|t| t.name == "video"));
        assert!(tools.iter().any(|t| t.name == "memory"));
    }

    #[test]
    fn test_tool_execution_result_success() {
        let result = ToolExecutionResult::success(
            "test_tool",
            "Hello World".to_string(),
            100,
            ToolSource::Native,
        );

        assert!(result.success);
        assert_eq!(result.tool_name, "test_tool");
        assert_eq!(result.content, "Hello World");
        assert!(result.error.is_none());
        assert_eq!(result.execution_time_ms, 100);
        assert_eq!(result.source, ToolSource::Native);
    }

    #[test]
    fn test_tool_execution_result_error() {
        let result = ToolExecutionResult::error(
            "test_tool",
            "Something went wrong".to_string(),
            50,
            ToolSource::Mcp,
        );

        assert!(!result.success);
        assert_eq!(result.tool_name, "test_tool");
        assert!(result.content.is_empty());
        assert_eq!(result.error, Some("Something went wrong".to_string()));
        assert_eq!(result.execution_time_ms, 50);
        assert_eq!(result.source, ToolSource::Mcp);
    }
}
