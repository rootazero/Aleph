//! Lightweight handle to an `AlephToolServer`.

use serde_json::Value;

use super::repair::{call_with_repair_impl, try_repair_tool_name_impl};
use super::ToolMap;
use crate::dispatcher::ToolDefinition;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::{ToolRepairInfo, ToolUpdateInfo};

/// Lightweight handle to an `AlephToolServer`.
///
/// This handle can be cloned cheaply and shared across async tasks.
/// It provides the same functionality as the server itself.
///
/// # Example
///
/// ```rust,ignore
/// let server = AlephToolServer::new();
/// let handle = server.handle();
///
/// // Clone for multiple tasks
/// let handle2 = handle.clone();
/// tokio::spawn(async move {
///     handle2.call("tool_name", args).await
/// });
/// ```
#[derive(Clone)]
pub struct AlephToolServerHandle {
    pub(super) tools: ToolMap,
}

impl AlephToolServerHandle {
    /// Add a tool to the server.
    pub async fn add_tool(&self, tool: impl AlephToolDyn + 'static) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, Arc::new(tool));
    }

    /// Add a pre-boxed dynamic tool.
    pub async fn add_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, tool);
    }

    /// Replace or add a tool with explicit update semantics.
    pub async fn replace_tool(&self, tool: impl AlephToolDyn + 'static) -> ToolUpdateInfo {
        let name = tool.name().to_string();
        let new_description = tool.definition().description;

        let mut tools = self.tools.write().await;
        let old_tool = tools.insert(name.clone(), Arc::new(tool));

        ToolUpdateInfo {
            tool_name: name,
            was_replaced: old_tool.is_some(),
            old_description: old_tool.map(|t| t.definition().description),
            new_description,
        }
    }

    /// Replace or add a pre-boxed dynamic tool.
    pub async fn replace_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) -> ToolUpdateInfo {
        let name = tool.name().to_string();
        let new_description = tool.definition().description;

        let mut tools = self.tools.write().await;
        let old_tool = tools.insert(name.clone(), tool);

        ToolUpdateInfo {
            tool_name: name,
            was_replaced: old_tool.is_some(),
            old_description: old_tool.map(|t| t.definition().description),
            new_description,
        }
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

    /// List all registered tools as `Arc<dyn AlephToolDyn>`.
    ///
    /// Used by the minimal agent loop factory to wrap tools via adapters.
    pub async fn list_tools_arc(&self) -> Vec<Arc<dyn AlephToolDyn>> {
        self.tools.read().await.values().cloned().collect()
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
            .ok_or_else(|| AlephError::tool_not_found(name))?;

        // Clone the Arc to release the read lock before calling
        let tool = Arc::clone(tool);
        drop(tools);

        tool.call(args).await
    }

    /// Call a tool with automatic repair for common errors.
    ///
    /// See `AlephToolServer::call_with_repair` for details.
    pub async fn call_with_repair(
        &self,
        name: &str,
        args: Value,
    ) -> (Result<Value>, Option<ToolRepairInfo>) {
        call_with_repair_impl(&self.tools, name, args).await
    }

    /// Try to repair a tool name using various normalization strategies.
    pub async fn try_repair_tool_name(&self, name: &str) -> Option<String> {
        try_repair_tool_name_impl(&self.tools, name).await
    }

    /// Clear all tools from the server.
    pub async fn clear(&self) {
        self.tools.write().await.clear();
    }
}
