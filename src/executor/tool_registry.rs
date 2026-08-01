//! `ToolRegistry` — the tool lookup + execution trait for the agent loop.
//!
//! The production tool stack dispatches every tool call through this trait,
//! implemented by [`BuiltinToolRegistry`](super::BuiltinToolRegistry).
//! `RegistryToolAdapter` (see [`crate::tools::adapters::registry_adapter`])
//! wraps any `ToolRegistry` implementor as a `LoopTool`, which the gateway's
//! `ScopedToolService` then exposes to the harness.
//!
//! The handle accessors (`workspace_handle`, …) let the execution engine hand
//! workspace- and session-scoped context to tools without threading it
//! through every call signature.

use crate::sync_primitives::Arc;
use serde_json::Value;

use crate::error::Result;
use crate::tool_metadata::UnifiedTool;

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

    /// Get the shared workspace handle for workspace-aware tools (e.g., `memory_search`).
    ///
    /// The execution engine writes the active `workspace_id` to this handle after
    /// workspace resolution, so tools use the correct workspace by default.
    /// Returns None if no workspace-aware tools are registered.
    fn workspace_handle(&self) -> Option<Arc<tokio::sync::RwLock<String>>> {
        None
    }

    /// Get the shared per-agent `SmartRecallConfig` map for the `memory_search` tool.
    ///
    /// The execution engine writes the running agent's profile `SmartRecallConfig`
    /// under that agent's id so the `memory_search` tool can use Two-Phase Smart
    /// Recall. Per-agent entries (not one global slot) so concurrent runs of
    /// different agents cannot overwrite each other's profile mid-turn.
    fn smart_recall_config_handle(
        &self,
    ) -> Option<
        Arc<
            tokio::sync::RwLock<
                std::collections::HashMap<String, crate::config::types::profile::SmartRecallConfig>,
            >,
        >,
    > {
        None
    }

    /// Get the shared session context handle for agent management tools.
    ///
    /// The execution engine writes the current `channel/peer_id` here so agent
    /// tools can bind agent switches to the correct conversation.
    fn session_context_handle(
        &self,
    ) -> Option<Arc<tokio::sync::RwLock<crate::builtin_tools::agent_manage::SessionContext>>> {
        None
    }

    /// Get the shared session key handle for `memory_search` `scope=current_session`.
    ///
    /// The execution engine writes the active session's key string here after
    /// session resolution so `memory_search` can filter facts by session scope.
    fn session_key_handle(&self) -> Option<Arc<tokio::sync::RwLock<String>>> {
        None
    }
}
