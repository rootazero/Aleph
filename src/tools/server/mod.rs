//! Tool Server with Hot-Reload Support
//!
//! Provides a thread-safe tool registry that supports runtime
//! addition and removal of tools, including automatic tool name repair.
//!
//! Inspired by OpenCode's experimental_repairToolCall pattern.

mod handle;
mod ops;
mod repair;
#[cfg(test)]
mod tests;

pub use handle::AlephToolServerHandle;

use std::collections::HashMap;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::dispatcher::ToolDefinition;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::{ToolRepairInfo, ToolUpdateInfo};

use ops::*;
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
        let name = tool.name().to_string();
        let tool_arc = Arc::new(tool);
        let tools = self.tools.clone();
        futures::executor::block_on(async move {
            let mut guard = tools.write().await;
            guard.insert(name, tool_arc);
        });
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
        let name = tool.name().to_string();
        let tool_arc = Arc::from(tool);
        let tools = self.tools.clone();
        futures::executor::block_on(async move {
            let mut guard = tools.write().await;
            guard.insert(name, tool_arc);
        });
        self
    }

    // =========================================================================
    // Async operations — delegate to shared ops module
    // =========================================================================

    /// Add a tool to the server.
    ///
    /// If a tool with the same name already exists, it will be replaced.
    pub async fn add_tool(&self, tool: impl AlephToolDyn + 'static) {
        add_tool_impl(&self.tools, tool).await
    }

    /// Add a pre-boxed dynamic tool.
    ///
    /// Useful when the tool is already wrapped in Arc.
    pub async fn add_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) {
        add_tool_arc_impl(&self.tools, tool).await
    }

    /// Replace or add a tool with explicit update semantics.
    ///
    /// Unlike `add_tool()` which silently replaces, this method returns
    /// information about whether an existing tool was replaced.
    pub async fn replace_tool(&self, tool: impl AlephToolDyn + 'static) -> ToolUpdateInfo {
        replace_tool_arc_impl(&self.tools, Arc::new(tool)).await
    }

    /// Replace or add a pre-boxed dynamic tool.
    ///
    /// Arc version of `replace_tool()`.
    pub async fn replace_tool_arc(&self, tool: Arc<dyn AlephToolDyn>) -> ToolUpdateInfo {
        replace_tool_arc_impl(&self.tools, tool).await
    }

    /// Remove a tool by name.
    ///
    /// Returns `true` if a tool was removed, `false` if not found.
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

    /// List all registered tools as `(name, Arc<dyn AlephToolDyn>)` pairs.
    ///
    /// Used by Phase 2 `build_tool_service` to wrap each builtin in a
    /// `BuiltinHandler` and register it into the shared `ToolRegistry`.
    pub async fn all_builtin_handlers(&self) -> Vec<(String, Arc<dyn AlephToolDyn>)> {
        let guard = self.tools.read().await;
        guard
            .iter()
            .map(|(name, tool)| (name.clone(), Arc::clone(tool)))
            .collect()
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
    ///
    /// # Errors
    ///
    /// Returns `AlephError::ToolNotFound` if the tool doesn't exist.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        call_impl(&self.tools, name, args).await
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
        clear_impl(&self.tools).await
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
