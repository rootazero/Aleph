//! Builtin Tool Registry for Agent Loop
//!
//! This module provides a `BuiltinToolRegistry` that implements the `ToolRegistry` trait,
//! allowing the Agent Loop's SingleStepExecutor to directly invoke builtin tools without
//! going through rig's agent framework.
//!
//! # Safety Features
//!
//! The registry integrates with the Three-Layer Control architecture's CapabilityGate
//! to enforce capability-based access control on tool execution.
//!
//! # Usage
//!
//! ```ignore
//! use aethecore::executor::{BuiltinToolRegistry, SingleStepExecutor};
//! use aethecore::three_layer::{Capability, CapabilityGate};
//!
//! // Create registry with capability restrictions
//! let gate = CapabilityGate::new(vec![
//!     Capability::FileRead,
//!     Capability::WebSearch,
//! ]);
//! let registry = BuiltinToolRegistry::with_gate(gate);
//! let executor = SingleStepExecutor::new(Arc::new(registry));
//! ```

mod config;
mod definitions;
mod executors;
mod registry;

pub use config::BuiltinToolConfig;
pub use definitions::{
    create_tool_boxed, get_builtin_tool_names, is_builtin_tool, BuiltinToolDefinition,
    BUILTIN_TOOL_DEFINITIONS,
};
pub use registry::BuiltinToolRegistry;

// Re-import ToolRegistry from single_step for internal use
use super::ToolRegistry;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use crate::agents::sub_agents::SubAgentDispatcher;
    use crate::dispatcher::{ToolRegistry as DispatcherToolRegistry, ToolSource};
    use crate::three_layer::{Capability, CapabilityGate};

    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = BuiltinToolRegistry::new();

        // Verify all tools are registered
        assert!(registry.get_tool("search").is_some());
        assert!(registry.get_tool("web_fetch").is_some());
        assert!(registry.get_tool("youtube").is_some());
        assert!(registry.get_tool("file_ops").is_some());
        assert!(registry.get_tool("code_exec").is_some());
        assert!(registry.get_tool("pdf_generate").is_some());

        // Verify unknown tool returns None
        assert!(registry.get_tool("unknown").is_none());
    }

    #[test]
    fn test_tool_metadata() {
        let registry = BuiltinToolRegistry::new();

        let search = registry.get_tool("search").unwrap();
        assert_eq!(search.name, "search");
        assert_eq!(search.id, "builtin:search");
        assert!(matches!(search.source, ToolSource::Builtin));
    }

    #[tokio::test]
    async fn test_unknown_tool_execution() {
        let registry = BuiltinToolRegistry::new();

        let result = registry
            .execute_tool("nonexistent", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown tool"));
    }

    #[test]
    fn test_required_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // Search requires WebSearch
        assert_eq!(
            registry.required_capability("search", &serde_json::json!({})),
            Some(Capability::WebSearch)
        );

        // Web fetch requires WebFetch
        assert_eq!(
            registry.required_capability("web_fetch", &serde_json::json!({})),
            Some(Capability::WebFetch)
        );

        // File ops - read operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "read"})),
            Some(Capability::FileRead)
        );

        // File ops - list operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "list"})),
            Some(Capability::FileList)
        );

        // File ops - write operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "write"})),
            Some(Capability::FileWrite)
        );

        // File ops - delete operation
        assert_eq!(
            registry.required_capability("file_ops", &serde_json::json!({"operation": "delete"})),
            Some(Capability::FileDelete)
        );
    }

    #[tokio::test]
    async fn test_capability_check_denied() {
        // Create registry with only WebSearch capability
        let gate = CapabilityGate::new(vec![Capability::WebSearch]);
        let registry = BuiltinToolRegistry::with_gate(gate);

        // Search should work (WebSearch granted)
        let search_result = registry.check_capability("search", &serde_json::json!({}));
        assert!(search_result.is_ok());

        // File ops read should fail (FileRead not granted)
        let file_result =
            registry.check_capability("file_ops", &serde_json::json!({"operation": "read"}));
        assert!(file_result.is_err());
        let err_msg = file_result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Permission denied") || err_msg.contains("capability"),
            "Expected permission error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_file_delete_allowed_by_default() {
        // Default registry now grants FileDelete for super-powered AI Agent
        let registry = BuiltinToolRegistry::new();

        // Delete capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "delete"}));
        assert!(check.is_ok(), "FileDelete should be allowed by default");
    }

    #[tokio::test]
    async fn test_code_exec_allowed_by_default() {
        // Default registry grants ShellExec for code execution
        let registry = BuiltinToolRegistry::new();

        // Code execution capability check should pass
        let check = registry.check_capability("code_exec", &serde_json::json!({}));
        assert!(check.is_ok(), "ShellExec should be allowed by default");
    }

    #[test]
    fn test_code_exec_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // Code exec requires ShellExec
        assert_eq!(
            registry.required_capability("code_exec", &serde_json::json!({})),
            Some(Capability::ShellExec)
        );
    }

    #[tokio::test]
    async fn test_file_read_allowed_by_default() {
        // Default registry grants FileRead
        let registry = BuiltinToolRegistry::new();

        // Read capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "read"}));
        assert!(check.is_ok());
    }

    #[tokio::test]
    async fn test_file_write_allowed_by_default() {
        // Default registry grants FileWrite for AI Agent tasks
        let registry = BuiltinToolRegistry::new();

        // Write capability check should pass
        let check = registry.check_capability("file_ops", &serde_json::json!({"operation": "write"}));
        assert!(check.is_ok());

        // Other write-like operations should also pass
        let check_mkdir = registry.check_capability("file_ops", &serde_json::json!({"operation": "mkdir"}));
        assert!(check_mkdir.is_ok());

        let check_copy = registry.check_capability("file_ops", &serde_json::json!({"operation": "copy"}));
        assert!(check_copy.is_ok());
    }

    #[test]
    fn test_pdf_generate_capability_mapping() {
        let registry = BuiltinToolRegistry::new();

        // PDF generate requires FileWrite
        assert_eq!(
            registry.required_capability("pdf_generate", &serde_json::json!({})),
            Some(Capability::FileWrite)
        );
    }

    #[tokio::test]
    async fn test_pdf_generate_allowed_by_default() {
        // Default registry grants FileWrite for PDF generation
        let registry = BuiltinToolRegistry::new();

        // PDF generate capability check should pass
        let check = registry.check_capability("pdf_generate", &serde_json::json!({}));
        assert!(check.is_ok(), "FileWrite should be allowed for pdf_generate");
    }

    #[test]
    fn test_meta_tools_not_registered_without_dispatcher_registry() {
        // Without dispatcher registry, meta tools should not be registered
        let registry = BuiltinToolRegistry::new();

        assert!(registry.get_tool("list_tools").is_none());
        assert!(registry.get_tool("get_tool_schema").is_none());
    }

    #[test]
    fn test_meta_tools_registered_with_dispatcher_registry() {
        // With dispatcher registry, meta tools should be registered
        let dispatcher_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let config = BuiltinToolConfig {
            dispatcher_registry: Some(dispatcher_registry),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert!(registry.get_tool("list_tools").is_some());
        assert!(registry.get_tool("get_tool_schema").is_some());
    }

    #[test]
    fn test_meta_tools_no_special_capability() {
        // Meta tools should not require any special capability
        let dispatcher_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let config = BuiltinToolConfig {
            dispatcher_registry: Some(dispatcher_registry),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert_eq!(
            registry.required_capability("list_tools", &serde_json::json!({})),
            None
        );
        assert_eq!(
            registry.required_capability("get_tool_schema", &serde_json::json!({})),
            None
        );
    }

    #[test]
    fn test_delegate_tool_not_registered_without_dispatcher() {
        // Without sub_agent_dispatcher, delegate tool should not be registered
        let registry = BuiltinToolRegistry::new();

        assert!(registry.get_tool("delegate").is_none());
    }

    #[test]
    fn test_delegate_tool_registered_with_dispatcher() {
        // With sub_agent_dispatcher, delegate tool should be registered
        let tool_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let sub_agent_dispatcher = Arc::new(RwLock::new(
            SubAgentDispatcher::with_defaults(tool_registry)
        ));
        let config = BuiltinToolConfig {
            sub_agent_dispatcher: Some(sub_agent_dispatcher),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert!(registry.get_tool("delegate").is_some());
        let delegate = registry.get_tool("delegate").unwrap();
        assert_eq!(delegate.name, "delegate");
        assert_eq!(delegate.id, "builtin:delegate");
    }

    #[test]
    fn test_delegate_tool_no_special_capability() {
        // Delegate tool should not require any special capability
        let tool_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let sub_agent_dispatcher = Arc::new(RwLock::new(
            SubAgentDispatcher::with_defaults(tool_registry)
        ));
        let config = BuiltinToolConfig {
            sub_agent_dispatcher: Some(sub_agent_dispatcher),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        assert_eq!(
            registry.required_capability("delegate", &serde_json::json!({})),
            None
        );
    }

    #[tokio::test]
    async fn test_delegate_tool_execution() {
        // With sub_agent_dispatcher, delegate tool should execute
        let tool_registry = Arc::new(RwLock::new(DispatcherToolRegistry::new()));
        let sub_agent_dispatcher = Arc::new(RwLock::new(
            SubAgentDispatcher::with_defaults(tool_registry)
        ));
        let config = BuiltinToolConfig {
            sub_agent_dispatcher: Some(sub_agent_dispatcher),
            ..Default::default()
        };
        let registry = BuiltinToolRegistry::with_config(config);

        // Execute delegate tool
        let result = registry.execute_tool(
            "delegate",
            serde_json::json!({
                "prompt": "List available MCP tools",
                "agent": "mcp"
            })
        ).await;

        // Should succeed (even with no tools available, it returns info about available servers)
        assert!(result.is_ok());
    }

    // ========================================================================
    // Sessions Tools Tests (gateway feature only)
    // ========================================================================

    #[cfg(feature = "gateway")]
    mod sessions_tests {
        use super::*;
        use crate::gateway::a2a_policy::AgentToAgentPolicy;
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

        #[test]
        fn test_sessions_tools_not_registered_without_context() {
            // Without gateway_context, sessions tools should not be registered
            let registry = BuiltinToolRegistry::new();

            assert!(registry.get_tool("sessions_list").is_none());
            assert!(registry.get_tool("sessions_send").is_none());
        }

        #[test]
        fn test_sessions_tools_registered_with_context() {
            // With gateway_context, sessions tools should be registered
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config);

            assert!(registry.get_tool("sessions_list").is_some());
            assert!(registry.get_tool("sessions_send").is_some());

            // Check tool metadata
            let sessions_list = registry.get_tool("sessions_list").unwrap();
            assert_eq!(sessions_list.name, "sessions_list");
            assert_eq!(sessions_list.id, "builtin:sessions_list");

            let sessions_send = registry.get_tool("sessions_send").unwrap();
            assert_eq!(sessions_send.name, "sessions_send");
            assert_eq!(sessions_send.id, "builtin:sessions_send");
        }

        #[test]
        fn test_sessions_tools_no_special_capability() {
            // Sessions tools should not require any special capability
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config);

            assert_eq!(
                registry.required_capability("sessions_list", &serde_json::json!({})),
                None
            );
            assert_eq!(
                registry.required_capability("sessions_send", &serde_json::json!({})),
                None
            );
        }

        #[tokio::test]
        async fn test_sessions_list_execution_without_context() {
            // Without gateway_context, sessions_list should fail with error
            let registry = BuiltinToolRegistry::new();

            let result = registry
                .execute_tool(
                    "sessions_list",
                    serde_json::json!({}),
                )
                .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("sessions_list not available"));
        }

        #[tokio::test]
        async fn test_sessions_list_execution_with_context() {
            // With gateway_context, sessions_list should execute successfully
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config);

            let result = registry
                .execute_tool(
                    "sessions_list",
                    serde_json::json!({}),
                )
                .await;

            assert!(result.is_ok());
            let output = result.unwrap();
            // Should return empty list since no sessions exist
            assert!(output.get("count").is_some());
            assert_eq!(output.get("count").unwrap().as_u64().unwrap(), 0);
        }

        #[tokio::test]
        async fn test_sessions_send_execution_without_context() {
            // Without gateway_context, sessions_send should fail with error
            let registry = BuiltinToolRegistry::new();

            let result = registry
                .execute_tool(
                    "sessions_send",
                    serde_json::json!({
                        "message": "Hello"
                    }),
                )
                .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("sessions_send not available"));
        }

        #[tokio::test]
        async fn test_sessions_send_execution_with_context() {
            // With gateway_context, sessions_send should execute
            // (though it may fail due to missing target agent)
            let gateway_context = create_test_gateway_context();
            let config = BuiltinToolConfig {
                gateway_context: Some(gateway_context),
                ..Default::default()
            };
            let registry = BuiltinToolRegistry::with_config(config);

            let result = registry
                .execute_tool(
                    "sessions_send",
                    serde_json::json!({
                        "message": "Hello",
                        "session_key": "agent:main:main"
                    }),
                )
                .await;

            // Should succeed but return an error status (agent not found)
            assert!(result.is_ok());
            let output = result.unwrap();
            assert!(output.get("status").is_some());
            // The status should be "error" since the target agent doesn't exist
            assert_eq!(output.get("status").unwrap().as_str().unwrap(), "error");
        }
    }
}
