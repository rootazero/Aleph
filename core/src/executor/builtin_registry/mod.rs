//! Builtin Tool Registry for Agent Loop
//!
//! This module provides a `BuiltinToolRegistry` that implements the `ToolRegistry` trait,
//! allowing the Agent Loop's SingleStepExecutor to directly invoke builtin tools without
//! going through rig's agent framework.
//!
//! # Safety Features
//!
//! Tool execution safety is enforced by:
//! - CommandChecker: Blocks dangerous shell commands (rm -rf /, sudo, etc.)
//! - PathPermissionChecker: Sandboxes file operations to allowed directories
//!
//! TODO: Tool policy will be reimplemented following OpenClaw's sandbox/tool-policy pattern.
//! See: /Volumes/TBU4/Workspace/openclaw/src/agents/sandbox/
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::executor::{BuiltinToolRegistry, SingleStepExecutor};
//!
//! let registry = BuiltinToolRegistry::new();
//! let executor = SingleStepExecutor::new(Arc::new(registry));
//! ```

mod builder;
mod config;
mod definitions;
mod executors;
mod groups;
mod registry;

pub use config::BuiltinToolConfig;
pub use definitions::{
    create_tool_boxed, get_builtin_tool_names, is_builtin_tool, BuiltinToolDefinition,
    BUILTIN_TOOL_DEFINITIONS,
};
pub use groups::TOOL_CATEGORIES;
pub use registry::BuiltinToolRegistry;

// Re-import ToolRegistry from single_step for internal use
use super::ToolRegistry;

#[cfg(test)]
mod tests {
    use crate::sync_primitives::Arc;

    use tokio::sync::RwLock;

    use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource};

    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = BuiltinToolRegistry::new().await;

        // Verify all tools are registered
        assert!(registry.get_tool("search").is_some());
        assert!(registry.get_tool("web_fetch").is_some());
        assert!(registry.get_tool("file_ops").is_some());
        assert!(registry.get_tool("code_exec").is_some());
        assert!(registry.get_tool("pdf_generate").is_some());
        assert!(registry.get_tool("desktop").is_some());

        // Verify unknown tool returns None
        assert!(registry.get_tool("unknown").is_none());
    }

    #[tokio::test]
    async fn test_tool_metadata() {
        let registry = BuiltinToolRegistry::new().await;

        let search = registry.get_tool("search").unwrap();
        assert_eq!(search.name, "search");
        assert_eq!(search.id, "builtin:search");
        assert!(matches!(search.source, ToolSource::Builtin));
    }

    #[tokio::test]
    async fn test_unknown_tool_returns_error() {
        let registry = BuiltinToolRegistry::new().await;

        let result = registry
            .execute_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown tool"));
    }

    // TODO: Capability tests removed - will be reimplemented with OpenClaw-style tool policy
    // See: /Volumes/TBU4/Workspace/openclaw/src/agents/pi-tools.policy.ts

    #[tokio::test]
    async fn test_capability_check_allows_all() {
        // Currently all operations are permitted (capability system removed)
        // Safety is enforced by CommandChecker and PathPermissionChecker
        let registry = BuiltinToolRegistry::new().await;

        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "delete"}));
        assert!(check.is_ok(), "All operations should be allowed currently");
    }

    #[tokio::test]
    async fn test_meta_tools_not_registered_without_dispatcher_registry() {
        // Without dispatcher registry, meta tools should not be registered
        let registry = BuiltinToolRegistry::new().await;

        assert!(registry.get_tool("list_tools").is_none());
        assert!(registry.get_tool("get_tool_schema").is_none());
    }

    #[tokio::test]
    async fn test_meta_tools_registered_with_dispatcher_registry() {
        // With dispatcher registry, meta tools should be registered
        let dispatcher_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let config = BuiltinToolConfig {
            dispatcher_registry: Some(dispatcher_registry),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config).await;

        assert!(registry.get_tool("list_tools").is_some());
        assert!(registry.get_tool("get_tool_schema").is_some());
    }

    // ========================================================================
    // Sessions Tools Tests (gateway feature only)
    // ========================================================================

    mod sessions_tests {
        use super::*;
        use crate::gateway::inter_agent_policy::AgentToAgentPolicy;
        use crate::gateway::agent_instance::AgentRegistry;
        use crate::gateway::context::GatewayContext;
        use crate::gateway::execution_adapter::ExecutionAdapter;
        use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
        use crate::gateway::event_emitter::EventEmitter;
        use crate::gateway::session_manager::SessionManagerConfig;
        use crate::gateway::{SessionManager, AgentInstance};
        use async_trait::async_trait;
        use tempfile::tempdir;

        /// Mock execution adapter for testing
        struct MockExecutionAdapter;

        #[async_trait]
        impl ExecutionAdapter for MockExecutionAdapter {
            async fn execute(
                &self,
                _request: RunRequest,
                _agent: Arc<AgentInstance>,
                _emitter: Arc<dyn EventEmitter + Send + Sync>,
            ) -> std::result::Result<(), ExecutionError> {
                Ok(())
            }

            async fn cancel(&self, run_id: &str) -> std::result::Result<(), ExecutionError> {
                Err(ExecutionError::RunNotFound(run_id.to_string()))
            }

            async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
                Some(RunStatus {
                    run_id: run_id.to_string(),
                    state: RunState::Completed,
                    started_at: Some(chrono::Utc::now()),
                    completed_at: Some(chrono::Utc::now()),
                    steps_completed: 0,
                    current_tool: None,
                })
            }
        }

        fn create_test_gateway_context() -> Arc<GatewayContext> {
            let temp = tempdir().unwrap();
            let session_config = SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            };
            let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
            let agent_registry = Arc::new(AgentRegistry::new());
            let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
            let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

            Arc::new(GatewayContext::new(
                session_manager,
                agent_registry,
                execution_adapter,
                a2a_policy,
            ))
        }

        #[tokio::test]
        async fn test_sessions_tools_always_registered_metadata() {
            // Sessions tool metadata is always registered (so LLM sees them),
            // but execution fails without GatewayContext injection.
            let registry = BuiltinToolRegistry::new().await;

            // Metadata present
            assert!(registry.get_tool("session_list").is_some());
            assert!(registry.get_tool("session_send").is_some());

            // Execution fails without GatewayContext
            let result = registry
                .execute_tool("session_list", serde_json::json!({}))
                .await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_sessions_tools_registered_with_context() {
            // With gateway_context, sessions tools should be registered
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config).await;

            assert!(registry.get_tool("session_list").is_some());
            assert!(registry.get_tool("session_send").is_some());

            // Check tool metadata
            let sessions_list = registry.get_tool("session_list").unwrap();
            assert_eq!(sessions_list.name, "session_list");
            assert_eq!(sessions_list.id, "builtin:session_list");

            let sessions_send = registry.get_tool("session_send").unwrap();
            assert_eq!(sessions_send.name, "session_send");
            assert_eq!(sessions_send.id, "builtin:session_send");
        }

        #[tokio::test]
        async fn test_sessions_list_execution_without_context() {
            // Without gateway_context, session.list should fail with error
            let registry = BuiltinToolRegistry::new().await;

            let result = registry
                .execute_tool(
                    "session_list",
                    serde_json::json!({}),
                )
                .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("not available") || err.to_string().contains("not yet injected"));
        }

        #[tokio::test]
        async fn test_sessions_list_execution_with_context() {
            // With gateway_context, session.list should execute successfully
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config).await;

            let result = registry
                .execute_tool(
                    "session_list",
                    serde_json::json!({}),
                )
                .await;

            assert!(result.is_ok());
        }
    }
}
