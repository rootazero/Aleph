//! Lightweight handle to an `AlephToolServer`.

use serde_json::Value;

use super::ops::*;
use super::repair::{call_with_repair_impl, try_repair_tool_name_impl};
use super::ToolMap;
use crate::dispatcher::ToolDefinition;
use crate::error::Result;
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
        add_tool_impl(&self.tools, tool).await
    }

    /// Add a pre-boxed dynamic tool.
    pub async fn add_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) {
        add_tool_arc_impl(&self.tools, tool).await
    }

    /// Replace or add a tool with explicit update semantics.
    pub async fn replace_tool(&self, tool: impl AlephToolDyn + 'static) -> ToolUpdateInfo {
        replace_tool_arc_impl(&self.tools, Arc::new(tool)).await
    }

    /// Replace or add a pre-boxed dynamic tool.
    pub async fn replace_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) -> ToolUpdateInfo {
        replace_tool_arc_impl(&self.tools, tool).await
    }

    /// Remove a tool by name.
    pub async fn remove_tool(&self, name: &str) -> bool {
        remove_tool_impl(&self.tools, name).await
    }

    /// Check if a tool exists.
    pub async fn has_tool(&self, name: &str) -> bool {
        has_tool_impl(&self.tools, name).await
    }

    /// Get the definition for a specific tool.
    pub async fn get_definition(&self, name: &str) -> Option<ToolDefinition> {
        get_definition_impl(&self.tools, name).await
    }

    /// List all tool definitions.
    pub async fn list_definitions(&self) -> Vec<ToolDefinition> {
        list_definitions_impl(&self.tools).await
    }

    /// List all tool names.
    pub async fn list_names(&self) -> Vec<String> {
        list_names_impl(&self.tools).await
    }

    /// List all registered tools as `Arc<dyn AlephToolDyn>`.
    ///
    /// Used by the minimal agent loop factory to wrap tools via adapters.
    pub async fn list_tools_arc(&self) -> Vec<Arc<dyn AlephToolDyn>> {
        list_tools_arc_impl(&self.tools).await
    }

    /// Get the number of registered tools.
    pub async fn len(&self) -> usize {
        len_impl(&self.tools).await
    }

    /// Check if the server has no tools.
    pub async fn is_empty(&self) -> bool {
        is_empty_impl(&self.tools).await
    }

    /// Call a tool by name with JSON arguments.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        call_impl(&self.tools, name, args).await
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
        clear_impl(&self.tools).await
    }
}
