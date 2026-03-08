//! Gateway Context
//!
//! Provides a shared context struct containing references to core gateway
//! components. This is passed to session tools (sessions_list, sessions_send)
//! via `BuiltinToolConfig` to enable agent-to-agent communication.

use crate::sync_primitives::Arc;

use super::inter_agent_policy::AgentToAgentPolicy;
use super::agent_instance::AgentRegistry;
use super::execution_adapter::ExecutionAdapter;
use super::session_manager::SessionManager;

/// Gateway context containing references to core components.
///
/// This struct is designed to be passed to builtin tools that need access
/// to gateway functionality, particularly for agent-to-agent communication.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::gateway::{
///     GatewayContext, SessionManager, AgentRegistry,
///     ExecutionAdapter, AgentToAgentPolicy,
/// };
/// use std::sync::Arc;
///
/// // Create context from existing components
/// let context = GatewayContext::new(
///     session_manager,
///     agent_registry,
///     execution_adapter,
///     a2a_policy,
/// );
///
/// // Pass to tool configuration
/// let tool_config = BuiltinToolConfig::with_gateway_context(context);
/// ```
#[derive(Clone)]
pub struct GatewayContext {
    /// Session manager for persisting and querying sessions
    session_manager: Arc<SessionManager>,

    /// Registry of all agent instances
    agent_registry: Arc<AgentRegistry>,

    /// Execution adapter for running agent requests
    execution_adapter: Arc<dyn ExecutionAdapter>,

    /// Policy controlling agent-to-agent communication
    a2a_policy: Arc<AgentToAgentPolicy>,
}

impl GatewayContext {
    /// Create a new gateway context.
    ///
    /// # Arguments
    ///
    /// * `session_manager` - Session manager for session persistence
    /// * `agent_registry` - Registry of agent instances
    /// * `execution_adapter` - Adapter for executing agent runs
    /// * `a2a_policy` - Policy for agent-to-agent communication
    pub fn new(
        session_manager: Arc<SessionManager>,
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
        a2a_policy: Arc<AgentToAgentPolicy>,
    ) -> Self {
        Self {
            session_manager,
            agent_registry,
            execution_adapter,
            a2a_policy,
        }
    }

    /// Get a reference to the session manager.
    pub fn session_manager(&self) -> &Arc<SessionManager> {
        &self.session_manager
    }

    /// Get a reference to the agent registry.
    pub fn agent_registry(&self) -> &Arc<AgentRegistry> {
        &self.agent_registry
    }

    /// Get a reference to the execution adapter.
    pub fn execution_adapter(&self) -> &Arc<dyn ExecutionAdapter> {
        &self.execution_adapter
    }

    /// Get a reference to the A2A policy.
    pub fn a2a_policy(&self) -> &Arc<AgentToAgentPolicy> {
        &self.a2a_policy
    }
}

impl std::fmt::Debug for GatewayContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayContext")
            .field("session_manager", &"Arc<SessionManager>")
            .field("agent_registry", &"Arc<AgentRegistry>")
            .field("execution_adapter", &"Arc<dyn ExecutionAdapter>")
            .field("a2a_policy", &self.a2a_policy)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_instance::AgentInstance;
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunStatus, RunState};
    use crate::gateway::session_manager::SessionManagerConfig;
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
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
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

    #[tokio::test]
    async fn test_gateway_context_creation() {
        let temp = tempdir().unwrap();

        let session_config = SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());

        let agent_registry = Arc::new(AgentRegistry::new());

        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);

        let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

        let context = GatewayContext::new(
            session_manager.clone(),
            agent_registry.clone(),
            execution_adapter.clone(),
            a2a_policy.clone(),
        );

        // Verify getters return the same Arc instances
        assert!(Arc::ptr_eq(context.session_manager(), &session_manager));
        assert!(Arc::ptr_eq(context.agent_registry(), &agent_registry));
        assert!(Arc::ptr_eq(context.a2a_policy(), &a2a_policy));
    }

    #[tokio::test]
    async fn test_gateway_context_clone() {
        let temp = tempdir().unwrap();

        let session_config = SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
        let agent_registry = Arc::new(AgentRegistry::new());
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

        let context = GatewayContext::new(
            session_manager.clone(),
            agent_registry.clone(),
            execution_adapter.clone(),
            a2a_policy.clone(),
        );

        let cloned = context.clone();

        // Both should point to the same underlying resources
        assert!(Arc::ptr_eq(context.session_manager(), cloned.session_manager()));
        assert!(Arc::ptr_eq(context.agent_registry(), cloned.agent_registry()));
        assert!(Arc::ptr_eq(context.a2a_policy(), cloned.a2a_policy()));
    }

    #[test]
    fn test_gateway_context_debug() {
        let temp = tempdir().unwrap();

        let session_config = SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
        let agent_registry = Arc::new(AgentRegistry::new());
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

        let context = GatewayContext::new(
            session_manager,
            agent_registry,
            execution_adapter,
            a2a_policy,
        );

        let debug_str = format!("{:?}", context);
        assert!(debug_str.contains("GatewayContext"));
        assert!(debug_str.contains("session_manager"));
        assert!(debug_str.contains("agent_registry"));
        assert!(debug_str.contains("execution_adapter"));
        assert!(debug_str.contains("a2a_policy"));
    }
}
