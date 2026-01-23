//! Tool Server with Hot-Reload Support
//!
//! Provides a thread-safe tool registry that supports runtime
//! addition and removal of tools.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::traits::AetherToolDyn;
use crate::dispatcher::ToolDefinition;
use crate::error::{AetherError, Result};

// =============================================================================
// AetherToolServer
// =============================================================================

/// Thread-safe tool server with hot-reload support.
///
/// This server manages a collection of tools that can be added, removed,
/// and invoked at runtime. It's designed for:
///
/// - MCP tool management (tools loaded from external processes)
/// - Plugin tool registration
/// - Dynamic tool discovery and hot-reload
///
/// # Thread Safety
///
/// All operations are thread-safe via `RwLock`. Multiple readers can
/// access tool definitions concurrently, while modifications are serialized.
///
/// # Example
///
/// ```rust,ignore
/// use crate::tools::{AetherToolServer, AetherTool};
///
/// let server = AetherToolServer::new();
///
/// // Add a tool
/// server.add_tool(SearchTool::new()).await;
///
/// // List all tools
/// let definitions = server.list_definitions().await;
///
/// // Call a tool
/// let result = server.call("search", serde_json::json!({"query": "rust"})).await?;
///
/// // Get a handle for sharing across tasks
/// let handle = server.handle();
/// tokio::spawn(async move {
///     handle.call("search", args).await
/// });
/// ```
pub struct AetherToolServer {
    tools: Arc<RwLock<HashMap<String, Arc<dyn AetherToolDyn>>>>,
}

impl AetherToolServer {
    /// Create a new empty tool server.
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Builder method to add a tool (sync, for construction).
    ///
    /// This method is useful for chaining during server construction:
    /// ```rust,ignore
    /// let server = AetherToolServer::new()
    ///     .tool(SearchTool::new())
    ///     .tool(WebFetchTool::new());
    /// ```
    pub fn tool(self, tool: impl AetherToolDyn + 'static) -> Self {
        // Get mutable access synchronously during construction
        // Safe because we own the server and no other references exist
        if let Ok(mut tools) = self.tools.try_write() {
            let name = tool.name().to_string();
            tools.insert(name, Arc::new(tool));
        }
        self
    }

    /// Add a tool to the server.
    ///
    /// If a tool with the same name already exists, it will be replaced.
    pub async fn add_tool(&self, tool: impl AetherToolDyn + 'static) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, Arc::new(tool));
    }

    /// Add a pre-boxed dynamic tool.
    ///
    /// Useful when the tool is already wrapped in Arc.
    pub async fn add_tool_arc(&self, tool: Arc<dyn AetherToolDyn>) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, tool);
    }

    /// Remove a tool by name.
    ///
    /// Returns `true` if a tool was removed, `false` if not found.
    pub async fn remove_tool(&self, name: &str) -> bool {
        self.tools.write().await.remove(name).is_some()
    }

    /// Check if a tool exists.
    pub async fn has_tool(&self, name: &str) -> bool {
        self.tools.read().await.contains_key(name)
    }

    /// Get the definition for a specific tool.
    pub async fn get_definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.read().await.get(name).map(|t| t.definition())
    }

    /// List all tool definitions.
    pub async fn list_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .await
            .values()
            .map(|t| t.definition())
            .collect()
    }

    /// List all tool names.
    pub async fn list_names(&self) -> Vec<String> {
        self.tools.read().await.keys().cloned().collect()
    }

    /// Get the number of registered tools.
    pub async fn len(&self) -> usize {
        self.tools.read().await.len()
    }

    /// Check if the server has no tools.
    pub async fn is_empty(&self) -> bool {
        self.tools.read().await.is_empty()
    }

    /// Call a tool by name with JSON arguments.
    ///
    /// # Errors
    ///
    /// Returns `AetherError::ToolNotFound` if the tool doesn't exist.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        let tools = self.tools.read().await;
        let tool = tools
            .get(name)
            .ok_or_else(|| AetherError::tool_not_found(name))?;

        // Clone the Arc to release the read lock before calling
        let tool = Arc::clone(tool);
        drop(tools);

        tool.call(args).await
    }

    /// Get a lightweight handle for sharing across tasks.
    ///
    /// The handle shares the same underlying tool registry and can be
    /// cloned cheaply for use in multiple async tasks.
    pub fn handle(&self) -> AetherToolServerHandle {
        AetherToolServerHandle {
            tools: Arc::clone(&self.tools),
        }
    }

    /// Clear all tools from the server.
    pub async fn clear(&self) {
        self.tools.write().await.clear();
    }
}

impl Default for AetherToolServer {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// AetherToolServerHandle
// =============================================================================

/// Lightweight handle to an `AetherToolServer`.
///
/// This handle can be cloned cheaply and shared across async tasks.
/// It provides the same functionality as the server itself.
///
/// # Example
///
/// ```rust,ignore
/// let server = AetherToolServer::new();
/// let handle = server.handle();
///
/// // Clone for multiple tasks
/// let handle2 = handle.clone();
/// tokio::spawn(async move {
///     handle2.call("tool_name", args).await
/// });
/// ```
#[derive(Clone)]
pub struct AetherToolServerHandle {
    tools: Arc<RwLock<HashMap<String, Arc<dyn AetherToolDyn>>>>,
}

impl AetherToolServerHandle {
    /// Add a tool to the server.
    pub async fn add_tool(&self, tool: impl AetherToolDyn + 'static) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, Arc::new(tool));
    }

    /// Add a pre-boxed dynamic tool.
    pub async fn add_tool_arc(&self, tool: Arc<dyn AetherToolDyn>) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, tool);
    }

    /// Remove a tool by name.
    pub async fn remove_tool(&self, name: &str) -> bool {
        self.tools.write().await.remove(name).is_some()
    }

    /// Check if a tool exists.
    pub async fn has_tool(&self, name: &str) -> bool {
        self.tools.read().await.contains_key(name)
    }

    /// Get the definition for a specific tool.
    pub async fn get_definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.read().await.get(name).map(|t| t.definition())
    }

    /// List all tool definitions.
    pub async fn list_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .await
            .values()
            .map(|t| t.definition())
            .collect()
    }

    /// List all tool names.
    pub async fn list_names(&self) -> Vec<String> {
        self.tools.read().await.keys().cloned().collect()
    }

    /// Get the number of registered tools.
    pub async fn len(&self) -> usize {
        self.tools.read().await.len()
    }

    /// Check if the server has no tools.
    pub async fn is_empty(&self) -> bool {
        self.tools.read().await.is_empty()
    }

    /// Call a tool by name with JSON arguments.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        let tools = self.tools.read().await;
        let tool = tools
            .get(name)
            .ok_or_else(|| AetherError::tool_not_found(name))?;

        // Clone the Arc to release the read lock before calling
        let tool = Arc::clone(tool);
        drop(tools);

        tool.call(args).await
    }

    /// Clear all tools from the server.
    pub async fn clear(&self) {
        self.tools.write().await.clear();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A pure dynamic tool for testing (only implements AetherToolDyn, not AetherTool)
    /// This allows dynamic name configuration needed for server tests.
    struct DynamicMockTool {
        name: String,
    }

    impl AetherToolDyn for DynamicMockTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                &self.name,
                "A dynamic mock tool for testing",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    },
                    "required": ["input"]
                }),
                crate::dispatcher::ToolCategory::Builtin,
            )
        }

        fn call(
            &self,
            args: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>>
        {
            let name = self.name.clone();
            Box::pin(async move {
                let input = args
                    .get("input")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(serde_json::json!({ "output": format!("{}: {}", name, input) }))
            })
        }
    }

    #[tokio::test]
    async fn test_server_add_and_call() {
        let server = AetherToolServer::new();

        server
            .add_tool(DynamicMockTool {
                name: "test".to_string(),
            })
            .await;

        assert!(server.has_tool("test").await);
        assert_eq!(server.len().await, 1);

        let result = server
            .call("test", serde_json::json!({"input": "hello"}))
            .await
            .unwrap();

        assert_eq!(result["output"], "test: hello");
    }

    #[tokio::test]
    async fn test_server_remove_tool() {
        let server = AetherToolServer::new();

        server
            .add_tool(DynamicMockTool {
                name: "removable".to_string(),
            })
            .await;

        assert!(server.has_tool("removable").await);
        assert!(server.remove_tool("removable").await);
        assert!(!server.has_tool("removable").await);
        assert!(!server.remove_tool("nonexistent").await);
    }

    #[tokio::test]
    async fn test_server_list_tools() {
        let server = AetherToolServer::new();

        server
            .add_tool(DynamicMockTool {
                name: "tool1".to_string(),
            })
            .await;
        server
            .add_tool(DynamicMockTool {
                name: "tool2".to_string(),
            })
            .await;

        let names = server.list_names().await;
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"tool1".to_string()));
        assert!(names.contains(&"tool2".to_string()));

        let definitions = server.list_definitions().await;
        assert_eq!(definitions.len(), 2);
    }

    #[tokio::test]
    async fn test_server_tool_not_found() {
        let server = AetherToolServer::new();

        let result = server
            .call("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AetherError::ToolNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn test_server_handle() {
        let server = AetherToolServer::new();
        let handle = server.handle();

        // Add via server
        server
            .add_tool(DynamicMockTool {
                name: "shared".to_string(),
            })
            .await;

        // Access via handle
        assert!(handle.has_tool("shared").await);

        let result = handle
            .call("shared", serde_json::json!({"input": "test"}))
            .await
            .unwrap();

        assert_eq!(result["output"], "shared: test");
    }

    #[tokio::test]
    async fn test_handle_clone() {
        let server = AetherToolServer::new();
        server
            .add_tool(DynamicMockTool {
                name: "cloned".to_string(),
            })
            .await;

        let handle1 = server.handle();
        let handle2 = handle1.clone();

        // Both handles see the same tools
        assert!(handle1.has_tool("cloned").await);
        assert!(handle2.has_tool("cloned").await);

        // Modifications via one handle are visible to the other
        handle1.remove_tool("cloned").await;
        assert!(!handle2.has_tool("cloned").await);
    }

    #[tokio::test]
    async fn test_server_clear() {
        let server = AetherToolServer::new();

        server
            .add_tool(DynamicMockTool {
                name: "t1".to_string(),
            })
            .await;
        server
            .add_tool(DynamicMockTool {
                name: "t2".to_string(),
            })
            .await;

        assert_eq!(server.len().await, 2);

        server.clear().await;

        assert!(server.is_empty().await);
    }
}
