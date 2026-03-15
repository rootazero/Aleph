//! Tool Server with Hot-Reload Support
//!
//! Provides a thread-safe tool registry that supports runtime
//! addition and removal of tools, including automatic tool name repair.
//!
//! Inspired by OpenCode's experimental_repairToolCall pattern.

mod handle;
mod repair;
#[cfg(test)]
mod tests;

pub use handle::AlephToolServerHandle;

use std::collections::HashMap;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::dispatcher::ToolDefinition;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::{ToolRepairInfo, ToolUpdateInfo};

use repair::{call_with_repair_impl, try_repair_tool_name_impl};

// =============================================================================
// Shared type alias
// =============================================================================

/// The shared, lock-protected tool registry used by both server and handle.
type ToolMap = Arc<RwLock<HashMap<String, Arc<dyn AlephToolDyn>>>>;

// =============================================================================
// AlephToolServer
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
/// use crate::tools::{AlephToolServer, AlephTool};
///
/// let server = AlephToolServer::new();
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
pub struct AlephToolServer {
    tools: ToolMap,
}

impl AlephToolServer {
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
    /// let server = AlephToolServer::new()
    ///     .tool(SearchTool::new())
    ///     .tool(WebFetchTool::new());
    /// ```
    pub fn tool(self, tool: impl AlephToolDyn + 'static) -> Self {
        // Get mutable access synchronously during construction
        // Safe because we own the server and no other references exist
        if let Ok(mut tools) = self.tools.try_write() {
            let name = tool.name().to_string();
            tools.insert(name, Arc::new(tool));
        }
        self
    }

    /// Add a boxed tool during construction (builder pattern).
    ///
    /// This method is useful when tools are created dynamically and already boxed.
    /// ```rust,ignore
    /// let tool = create_tool_boxed("search", &config);
    /// let server = AlephToolServer::new().tool_boxed(tool);
    /// ```
    pub fn tool_boxed(self, tool: Box<dyn AlephToolDyn>) -> Self {
        // Get mutable access synchronously during construction
        // Safe because we own the server and no other references exist
        if let Ok(mut tools) = self.tools.try_write() {
            let name = tool.name().to_string();
            tools.insert(name, Arc::from(tool));
        }
        self
    }

    /// Add a tool to the server.
    ///
    /// If a tool with the same name already exists, it will be replaced.
    pub async fn add_tool(&self, tool: impl AlephToolDyn + 'static) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, Arc::new(tool));
    }

    /// Add a pre-boxed dynamic tool.
    ///
    /// Useful when the tool is already wrapped in Arc.
    pub async fn add_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, tool);
    }

    /// Replace or add a tool with explicit update semantics.
    ///
    /// Unlike `add_tool()` which silently replaces, this method returns
    /// information about whether an existing tool was replaced.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let update_info = server.replace_tool(new_version).await;
    /// if update_info.was_replaced {
    ///     println!("Updated {} from v1 to v2", update_info.tool_name);
    /// } else {
    ///     println!("Added new tool: {}", update_info.tool_name);
    /// }
    /// ```
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
    ///
    /// Arc version of `replace_tool()`.
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
    ///
    /// # Errors
    ///
    /// Returns `AlephError::ToolNotFound` if the tool doesn't exist.
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
    /// This method attempts to:
    /// 1. Call the tool directly if found
    /// 2. Try case-insensitive matching if exact match fails
    /// 3. Route to "invalid" tool if no match found
    ///
    /// Inspired by OpenCode's experimental_repairToolCall pattern.
    ///
    /// # Returns
    ///
    /// A tuple of (result, repair_info) where repair_info is Some if a repair was made.
    pub async fn call_with_repair(
        &self,
        name: &str,
        args: Value,
    ) -> (Result<Value>, Option<ToolRepairInfo>) {
        call_with_repair_impl(&self.tools, name, args).await
    }

    /// Try to repair a tool name using various normalization strategies.
    ///
    /// Returns the repaired name if a match is found, None otherwise.
    pub async fn try_repair_tool_name(&self, name: &str) -> Option<String> {
        try_repair_tool_name_impl(&self.tools, name).await
    }

    /// Get a lightweight handle for sharing across tasks.
    ///
    /// The handle shares the same underlying tool registry and can be
    /// cloned cheaply for use in multiple async tasks.
    pub fn handle(&self) -> AlephToolServerHandle {
        AlephToolServerHandle {
            tools: Arc::clone(&self.tools),
        }
    }

    /// Clear all tools from the server.
    pub async fn clear(&self) {
        self.tools.write().await.clear();
    }

    /// Create a new tool server with Markdown skills loaded from directories.
    ///
    /// This method initializes the server and loads Markdown skills from
    /// the specified directories.
    ///
    /// # Arguments
    ///
    /// * `skill_dirs` - Directories to scan for SKILL.md files
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let server = AlephToolServer::new_with_skills(vec![
    ///     PathBuf::from("skills"),
    ///     PathBuf::from("~/.aleph/skills"),
    /// ]).await;
    /// ```
    pub async fn new_with_skills(skill_dirs: Vec<std::path::PathBuf>) -> Self {
        use tracing::{error, info};

        let server = Self::new();

        // Load Markdown skills from directories
        for dir in skill_dirs {
            info!(dir = %dir.display(), "Loading Markdown skills");
            let tools = crate::tools::markdown_skill::load_skills_from_dir(dir).await;

            for tool in tools {
                let name = tool.spec.name.clone();
                if let Err(e) = server.add_tool_dyn(Box::new(tool)).await {
                    error!(skill = %name, error = %e, "Failed to register skill");
                } else {
                    info!(skill = %name, "Registered Markdown skill");
                }
            }
        }

        server
    }

    /// Add a pre-boxed dynamic tool (internal helper).
    async fn add_tool_dyn(&self, tool: Box<dyn AlephToolDyn>) -> Result<()> {
        let name = tool.name().to_string();
        self.tools.write().await.insert(name, Arc::from(tool));
        Ok(())
    }
}

impl Default for AlephToolServer {
    fn default() -> Self {
        Self::new()
    }
}
