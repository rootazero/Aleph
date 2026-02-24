//! Execution Adapter
//!
//! Provides the `ExecutionAdapter` trait that abstracts over different execution engine
//! implementations. This allows `InboundMessageRouter` to work with either the full
//! `ExecutionEngine<P, R>` or the `SimpleExecutionEngine` without requiring generics.

use std::sync::Arc;

use async_trait::async_trait;

use super::agent_instance::AgentInstance;
use super::event_emitter::EventEmitter;
use super::execution_engine::{ExecutionError, RunRequest, RunStatus};

/// Trait for abstracting over execution engine implementations.
///
/// This trait allows components like `InboundMessageRouter` to execute agent runs
/// without being generic over the specific execution engine type.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::gateway::{ExecutionAdapter, RunRequest};
///
/// async fn handle_message(
///     adapter: Arc<dyn ExecutionAdapter>,
///     request: RunRequest,
///     agent: Arc<AgentInstance>,
///     emitter: Arc<dyn EventEmitter + Send + Sync>,
/// ) -> Result<(), ExecutionError> {
///     adapter.execute(request, agent, emitter).await
/// }
/// ```
#[async_trait]
pub trait ExecutionAdapter: Send + Sync {
    /// Execute a run request with the given agent and event emitter.
    ///
    /// This starts an agent execution loop that will:
    /// 1. Accept the run and emit `RunAccepted` event
    /// 2. Process the input through the agent loop
    /// 3. Emit streaming events (reasoning, tool calls, response chunks)
    /// 4. Complete with `RunComplete` or `RunError` event
    ///
    /// # Arguments
    ///
    /// * `request` - The run request containing input, session key, and metadata
    /// * `agent` - The agent instance to execute against
    /// * `emitter` - Event emitter for streaming events back to clients
    ///
    /// # Errors
    ///
    /// Returns `ExecutionError` if:
    /// - Agent is busy (`AgentBusy`)
    /// - Too many concurrent runs (`TooManyRuns`)
    /// - Execution fails (`Failed`)
    /// - Run times out (`Timeout`)
    /// - Run is cancelled (`Cancelled`)
    async fn execute(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError>;

    /// Cancel a run by its ID.
    ///
    /// Sends a cancellation signal to the running execution. The run will
    /// complete with a `Cancelled` state and emit a `RunError` event.
    ///
    /// # Arguments
    ///
    /// * `run_id` - The unique identifier of the run to cancel
    ///
    /// # Errors
    ///
    /// Returns `ExecutionError` if:
    /// - Run is not found (`RunNotFound`)
    /// - Run is not active (`RunNotActive`)
    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError>;

    /// Get the status of a run by its ID.
    ///
    /// Returns the current status including state, timing, and progress info.
    /// Returns `None` if the run is not found or has been cleaned up.
    ///
    /// # Arguments
    ///
    /// * `run_id` - The unique identifier of the run to query
    async fn get_status(&self, run_id: &str) -> Option<RunStatus>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::gateway::execution_engine::RunState;
    use crate::gateway::router::SessionKey;

    /// Mock execution adapter for testing
    struct MockExecutionAdapter {
        should_fail: bool,
    }

    impl MockExecutionAdapter {
        fn new() -> Self {
            Self { should_fail: false }
        }

        #[allow(dead_code)]
        fn failing() -> Self {
            Self { should_fail: true }
        }
    }

    #[async_trait]
    impl ExecutionAdapter for MockExecutionAdapter {
        async fn execute(
            &self,
            request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            if self.should_fail {
                Err(ExecutionError::Failed("Mock failure".to_string()))
            } else {
                // Simulate successful execution
                tracing::info!("Mock executing run: {}", request.run_id);
                Ok(())
            }
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            // Mock: always return not found for simplicity
            Err(ExecutionError::RunNotFound(run_id.to_string()))
        }

        async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
            // Mock: return a completed status
            Some(RunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: Some(chrono::Utc::now()),
                completed_at: Some(chrono::Utc::now()),
                steps_completed: 1,
                current_tool: None,
            })
        }
    }

    #[tokio::test]
    async fn test_mock_adapter_execute() {
        use crate::gateway::agent_instance::AgentInstanceConfig;
        use crate::gateway::event_emitter::NoOpEventEmitter;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config).unwrap());
        let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(NoOpEventEmitter::new());
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter::new());

        let request = RunRequest {
            run_id: "test-run".to_string(),
            input: "Hello".to_string(),
            session_key: SessionKey::main("test"),
            timeout_secs: None,
            metadata: HashMap::new(),
        };

        let result = adapter.execute(request, agent, emitter).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_adapter_get_status() {
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter::new());

        let status = adapter.get_status("some-run").await;
        assert!(status.is_some());

        let status = status.unwrap();
        assert_eq!(status.run_id, "some-run");
        assert_eq!(status.state, RunState::Completed);
    }

    #[tokio::test]
    async fn test_mock_adapter_cancel() {
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter::new());

        let result = adapter.cancel("nonexistent").await;
        assert!(matches!(result, Err(ExecutionError::RunNotFound(_))));
    }
}
