//! Execution Adapter
//!
//! Provides the `ExecutionAdapter` trait that abstracts over different execution engine
//! implementations. This allows `InboundMessageRouter` to work with either the full
//! `ExecutionEngine<P, R>` or the `SimpleExecutionEngine` without requiring generics.

use crate::sync_primitives::Arc;

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

    /// Cancel the `Running` run on `session_key`, if any.
    ///
    /// Channel-facing variant of [`ExecutionAdapter::cancel`]: a channel user
    /// (`/stop`) knows their session, not the engine-internal run id. Returns
    /// the cancelled run's id, or `Ok(None)` when no run is active on the
    /// session — "nothing to stop" is an answer, not an error.
    ///
    /// Default implementation reports no active run, so adapters without a
    /// per-session run table (mocks, schedulers) need no changes.
    async fn cancel_session(
        &self,
        _session_key: &crate::routing::session_key::SessionKey,
    ) -> Result<Option<String>, ExecutionError> {
        Ok(None)
    }

    /// Get the status of a run by its ID.
    ///
    /// Returns the current status including state, timing, and progress info.
    /// Returns `None` if the run is not found or has been cleaned up.
    ///
    /// # Arguments
    ///
    /// * `run_id` - The unique identifier of the run to query
    async fn get_status(&self, run_id: &str) -> Option<RunStatus>;

    /// Get number of currently active runs
    async fn active_run_count(&self) -> usize;

    /// Snapshot of the run-lifetime `ConcurrencyLimiter`'s global slot usage,
    /// surfaced via `gateway.metrics.run_concurrency` (Task 8, audit 3.4) —
    /// "N/M run slots in use" for ops dashboards / Panel UIs.
    ///
    /// Default implementation returns an all-zero snapshot for adapters that
    /// have no `ConcurrencyLimiter` concept (mocks, `SimpleExecutionEngine`):
    /// a harmless "0 of 0 slots" reading rather than a mandatory override.
    fn concurrency_snapshot(&self) -> super::execution_engine::ConcurrencySnapshot {
        super::execution_engine::ConcurrencySnapshot::default()
    }

    /// Session keys with a run currently in flight (authoritative in-memory
    /// admission gate). Surfaced beside `concurrency_snapshot` via
    /// `gateway.metrics.run_concurrency` so Panels can paint per-session
    /// running indicators.
    ///
    /// Default implementation returns an empty set for adapters that have no
    /// per-session run registry (mocks, `SimpleExecutionEngine`).
    fn running_sessions(&self) -> Vec<String> {
        Vec::new()
    }

    /// The run currently in flight on ONE session key, if any.
    ///
    /// Point-lookup twin of [`Self::running_sessions`], and the reason it is a
    /// separate method rather than a scan of that vector: the set answers
    /// "which sessions are busy" while a client joining a session mid-turn
    /// needs "and which run is it", so it can bind that run id and render the
    /// rest of the turn live instead of watching a transcript that will not
    /// move until the run ends.
    ///
    /// Default implementation returns `None` for adapters with no per-session
    /// run registry (mocks, `SimpleExecutionEngine`) — the same degradation
    /// [`Self::running_sessions`] already makes, and it reads as "no live turn
    /// to join", which is the honest answer for an engine that cannot tell.
    fn active_run_for_session(&self, _session_key: &str) -> Option<String> {
        None
    }

    /// The session a live run belongs to — the inverse lookup of
    /// [`Self::active_run_for_session`], and the reason it exists is
    /// [`Self::cancel_session`].
    ///
    /// Stopping a run means stopping the work it owns. `cancel` fires exactly
    /// one run's token; `cancel_session` additionally walks the delegated
    /// children registered under that session's key, which is what keeps a
    /// cancelled leader from leaving its member runs detached and still
    /// billing. The channel `/stop` path always had the session key and so
    /// always got the walk; every run-id-addressed stop — Panel `chat.abort`,
    /// TUI `/stop`, `aleph chat abort` — went through `AgentRunManager::
    /// cancel_run`, which had only a run id and therefore only ever fired the
    /// leader's token. Both `cancel_session`'s own doc and FEATURE_LOCATOR
    /// §4.8 described `chat.abort` as taking the session path; it did not.
    ///
    /// With this lookup the run-id face resolves the session first and joins
    /// the same verb, so "stop" means the same thing on every surface. A run
    /// the engine does not hold — already finished, or still waiting in its
    /// busy lane — yields `None` and the caller falls back to the run-id
    /// primitive, which is also what reaches `busy_queue::cancel_queued_run`.
    ///
    /// Default implementation returns `None`, the same degradation
    /// [`Self::active_run_for_session`] makes: an adapter with no per-session
    /// run table keeps exactly its previous behaviour.
    async fn session_of_run(
        &self,
        _run_id: &str,
    ) -> Option<crate::routing::session_key::SessionKey> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::gateway::execution_engine::RunState;
    use crate::gateway::media::PendingMedia;
    use crate::gateway::router::SessionKey;

    /// Mock execution adapter for testing
    struct MockExecutionAdapter {
        should_fail: bool,
    }

    impl MockExecutionAdapter {
        fn new() -> Self {
            Self { should_fail: false }
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

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    #[tokio::test]
    async fn test_mock_adapter_execute() {
        use crate::gateway::agent_instance::AgentInstanceConfig;
        use crate::gateway::event_emitter::NoOpEventEmitter;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let sm_config = crate::gateway::session_manager::SessionManagerConfig {
            db_path: temp.path().join("test_sessions.db"),
            ..Default::default()
        };
        let sm = Arc::new(
            crate::gateway::session_manager::SessionManager::new(sm_config)
                .expect("test session manager"),
        );
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            agent_dir: temp.path().join("agents/test"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config, sm).unwrap());
        let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(NoOpEventEmitter::new());
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter::new());

        let request = RunRequest {
            run_id: "test-run".to_string(),
            input: "Hello".to_string(),
            session_key: SessionKey::main("test"),
            timeout_secs: None,
            metadata: HashMap::new(),
            attachments: Vec::new(),
            pending_media: PendingMedia::default(),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
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
