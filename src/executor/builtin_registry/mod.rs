//! Builtin Tool Registry for Agent Loop
//!
//! This module provides a `BuiltinToolRegistry` that implements the
//! `ToolRegistry` trait, letting the agent loop's tool stack invoke builtin
//! tools directly without going through rig's agent framework.
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::executor::BuiltinToolRegistry;
//!
//! let registry = BuiltinToolRegistry::new().await;
//! ```

mod builder;
mod config;
mod definitions;
mod groups;
mod registry;

pub use config::BuiltinToolConfig;
pub use definitions::{create_tool_boxed, get_builtin_tool_names, BUILTIN_TOOL_DEFINITIONS};
/// Re-exported for `thinker::prompt_contract`, whose duplicate-sentence scan
/// ships-text surface is the same one the byte ratchet measures. Test-only:
/// it names no runtime behaviour, only what the guards are allowed to be
/// blind to.
#[cfg(test)]
pub(crate) use definitions::{
    BRIDGE_TOOL_DESCRIPTIONS, INJECTED_TOOL_DESCRIPTIONS, REGISTRY_ONLY_DESCRIPTIONS,
};
pub use groups::TOOL_CATEGORIES;
pub use registry::BuiltinToolRegistry;

// Re-import the ToolRegistry trait for the `impl ToolRegistry for ...` block.
use super::ToolRegistry;

#[cfg(test)]
mod tests {
    use crate::sync_primitives::Arc;

    use crate::tool_metadata::ToolSource;

    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = BuiltinToolRegistry::new().await.unwrap();

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
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = BuiltinToolRegistry::new().await.unwrap();

        let search = registry.get_tool("search").unwrap();
        assert_eq!(search.name, "search");
        assert_eq!(search.id, "builtin:search");
        assert!(matches!(search.source, ToolSource::Builtin));
    }

    #[tokio::test]
    async fn test_unknown_tool_returns_error() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let registry = BuiltinToolRegistry::new().await.unwrap();

        let result = registry
            .execute_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_resolve_plugin_handler_uses_extension_snapshot_for_dynamic_tool() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let manager = Arc::new(
            crate::extension::ExtensionManager::with_defaults()
                .await
                .unwrap(),
        );
        {
            let mut registry = manager.get_plugin_registry_mut().await;
            registry.register_plugin(crate::extension::PluginRecord::new(
                "dyn-plugin".to_string(),
                "Dynamic Plugin".to_string(),
                crate::extension::PluginKind::Static,
                crate::extension::PluginOrigin::Global,
            ));
            registry.register_tool(crate::extension::ToolRegistration {
                name: "dynamic_tool".to_string(),
                description: "Dynamic plugin tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                handler: "handle_dynamic_tool".to_string(),
                plugin_id: "dyn-plugin".to_string(),
            });
        }
        manager.sync_runtime_snapshots().await;

        assert_eq!(
            super::registry::resolve_plugin_handler_from_sources(
                Some(manager.as_ref()),
                &std::collections::HashMap::new(),
                "dynamic_tool",
            ),
            Some(("dyn-plugin".to_string(), "handle_dynamic_tool".to_string()))
        );
    }

    #[tokio::test]
    async fn test_resolve_plugin_handler_ignores_disabled_plugin_tool() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let manager = Arc::new(
            crate::extension::ExtensionManager::with_defaults()
                .await
                .unwrap(),
        );
        {
            let mut registry = manager.get_plugin_registry_mut().await;
            registry.register_plugin(crate::extension::PluginRecord::new(
                "disabled-plugin".to_string(),
                "Disabled Plugin".to_string(),
                crate::extension::PluginKind::Static,
                crate::extension::PluginOrigin::Global,
            ));
            registry.register_tool(crate::extension::ToolRegistration {
                name: "hidden_tool".to_string(),
                description: "Hidden plugin tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                handler: "handle_hidden_tool".to_string(),
                plugin_id: "disabled-plugin".to_string(),
            });
        }
        manager.set_plugin_enabled("disabled-plugin", false).await;

        assert_eq!(
            super::registry::resolve_plugin_handler_from_sources(
                Some(manager.as_ref()),
                &std::collections::HashMap::new(),
                "hidden_tool",
            ),
            None
        );
    }

    #[test]
    fn test_resolve_plugin_handler_falls_back_to_static_plugin_metadata() {
        let mut tools = std::collections::HashMap::new();
        tools.insert(
            "legacy_tool".to_string(),
            crate::tool_metadata::UnifiedTool::new(
                "plugin:legacy:legacy_tool",
                "legacy_tool",
                "Legacy plugin tool",
                crate::tool_metadata::ToolSource::Plugin {
                    plugin_id: "legacy-plugin".to_string(),
                },
            ),
        );

        assert_eq!(
            super::registry::resolve_plugin_handler_from_sources(None, &tools, "legacy_tool"),
            Some(("legacy-plugin".to_string(), "tool_legacy_tool".to_string()))
        );
    }

    // ========================================================================
    // Sessions Tools Tests (gateway feature only)
    // ========================================================================

    mod sessions_tests {
        use super::*;
        use crate::gateway::agent_instance::AgentRegistry;
        use crate::gateway::context::GatewayContext;
        use crate::gateway::event_emitter::EventEmitter;
        use crate::gateway::execution_adapter::ExecutionAdapter;
        use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
        use crate::gateway::inter_agent_policy::AgentToAgentPolicy;
        use crate::gateway::session_manager::SessionManagerConfig;
        use crate::gateway::{AgentInstance, SessionManager};
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

            async fn active_run_count(&self) -> usize {
                0
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
            let _home = crate::utils::paths::IsolatedAlephHome::new();
            let registry = BuiltinToolRegistry::new().await.unwrap();
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
            let _home = crate::utils::paths::IsolatedAlephHome::new();
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config).await.unwrap();

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
            let _home = crate::utils::paths::IsolatedAlephHome::new();
            let registry = BuiltinToolRegistry::new().await.unwrap();

            let result = registry
                .execute_tool("session_list", serde_json::json!({}))
                .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                err.to_string().contains("not available")
                    || err.to_string().contains("not yet injected")
            );
        }

        #[tokio::test]
        async fn test_sessions_list_execution_with_context() {
            // With gateway_context, session.list should execute successfully
            let _home = crate::utils::paths::IsolatedAlephHome::new();
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config).await.unwrap();

            let result = registry
                .execute_tool("session_list", serde_json::json!({}))
                .await;

            assert!(result.is_ok());
        }
    }
}
