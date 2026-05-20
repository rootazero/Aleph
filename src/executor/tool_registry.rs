//! `ToolRegistry` — the tool lookup + execution trait for the agent loop.
//!
//! The production tool stack dispatches every tool call through this trait,
//! implemented by [`BuiltinToolRegistry`](super::BuiltinToolRegistry).
//! `RegistryToolAdapter` (see [`crate::tools::adapters::registry_adapter`])
//! wraps any `ToolRegistry` implementor as a `LoopTool`, which the gateway's
//! `ScopedToolService` then exposes to the harness.
//!
//! The handle accessors (`workspace_handle`, `tool_context_handle`, …) let the
//! execution engine hand workspace- and session-scoped context to tools
//! without threading it through every call signature.

use crate::sync_primitives::Arc;
use serde_json::Value;

use crate::dispatcher::UnifiedTool;
use crate::error::Result;

/// Trait for tool registry lookup and execution.
pub trait ToolRegistry: Send + Sync {
    /// Look up a tool by name.
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool>;

    /// Execute a tool call.
    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>>;

    /// Get the shared workspace handle for workspace-aware tools (e.g., memory_search).
    ///
    /// The execution engine writes the active workspace_id to this handle after
    /// workspace resolution, so tools use the correct workspace by default.
    /// Returns None if no workspace-aware tools are registered.
    fn workspace_handle(&self) -> Option<Arc<tokio::sync::RwLock<String>>> {
        None
    }

    /// Get the shared SmartRecallConfig handle for the memory_search tool.
    ///
    /// The execution engine writes the active workspace profile's SmartRecallConfig
    /// here so the memory_search tool can use Two-Phase Smart Recall.
    fn smart_recall_config_handle(
        &self,
    ) -> Option<Arc<tokio::sync::RwLock<Option<crate::config::types::profile::SmartRecallConfig>>>>
    {
        None
    }

    /// Get the shared session context handle for agent management tools.
    ///
    /// The execution engine writes the current channel/peer_id here so agent
    /// tools can bind agent switches to the correct conversation.
    fn session_context_handle(
        &self,
    ) -> Option<Arc<tokio::sync::RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        None
    }

    /// Get the shared tool policy handle for per-agent tool access control.
    ///
    /// When set, execute_tool() checks this policy before dispatching.
    /// Default ToolPolicy (empty whitelist/blacklist) allows all tools.
    fn tool_policy_handle(
        &self,
    ) -> Option<Arc<tokio::sync::RwLock<crate::builtin_tools::agent_manage::ToolPolicy>>> {
        None
    }

    /// Get the shared tool context handle for workspace-scoped output paths.
    ///
    /// The execution engine writes the active agent's ToolContext here so
    /// tools that write output files use the correct workspace directory.
    fn tool_context_handle(&self) -> Option<crate::tools::ToolContextHandle> {
        None
    }

    /// Get the shared session key handle for memory_search scope=current_session.
    ///
    /// The execution engine writes the active session's key string here after
    /// session resolution so memory_search can filter facts by session scope.
    fn session_key_handle(&self) -> Option<Arc<tokio::sync::RwLock<String>>> {
        None
    }
}
