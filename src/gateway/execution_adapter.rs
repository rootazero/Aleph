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

/// Cancellation shared by a receipt's run entry and its execution, including admission.
#[derive(Clone, Debug, Default)]
pub struct ChatRunCancellation {
    token: tokio_util::sync::CancellationToken,
    state: Arc<std::sync::Mutex<ChatRunCancellationState>>,
    inline_settled: Arc<tokio::sync::Notify>,
    remote_clarification: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ChatRunCancellationState {
    #[default]
    Open,
    Requested,
    ClarificationRequested(String),
    Inline {
        stop_requested: bool,
    },
    Indeterminate,
    Finished {
        requested: bool,
    },
}

/// The stop intent was recorded, but the inline outcome cannot yet be acknowledged.
#[derive(Debug)]
pub(crate) struct ChatRunStopUndecided;

// Do not let a blocked inline append outlast the caller's RPC deadline.
const INLINE_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

impl ChatRunCancellation {
    pub(crate) fn for_remote_task() -> Self {
        Self {
            remote_clarification: true,
            ..Self::default()
        }
    }

    pub(crate) fn allows_remote_clarification(&self) -> bool {
        self.remote_clarification
    }

    /// Claim the stop boundary for clarification. A caller cancellation and a
    /// clarification can never both win this boundary.
    pub(crate) fn request_clarification(&self, question: String) -> Result<(), ExecutionError> {
        let mut state = self.state.lock().unwrap();
        if !self.remote_clarification
            || *state != ChatRunCancellationState::Open
            || self.token.is_cancelled()
        {
            return Err(ExecutionError::Cancelled);
        }
        *state = ChatRunCancellationState::ClarificationRequested(question);
        self.token.cancel();
        Ok(())
    }

    pub(crate) fn clarification(&self) -> Option<String> {
        match &*self.state.lock().unwrap() {
            ChatRunCancellationState::ClarificationRequested(question) => Some(question.clone()),
            _ => None,
        }
    }

    pub(crate) fn token(&self) -> tokio_util::sync::CancellationToken {
        self.token.clone()
    }

    pub(crate) async fn request(&self) -> Result<bool, ChatRunStopUndecided> {
        let mut owns_intent = false;
        tokio::time::timeout(INLINE_STOP_WAIT, async {
            loop {
                // Register before reading the state so settlement cannot lose its wake.
                let settled = self.inline_settled.notified();
                tokio::pin!(settled);
                settled.as_mut().enable();
                {
                    let mut state = self.state.lock().unwrap();
                    match &mut *state {
                        ChatRunCancellationState::Open if !self.token.is_cancelled() => {
                            *state = ChatRunCancellationState::Requested;
                            self.token.cancel();
                            return Ok(true);
                        }
                        ChatRunCancellationState::Inline { stop_requested } => {
                            if !*stop_requested {
                                *stop_requested = true;
                                owns_intent = true;
                            }
                        }
                        ChatRunCancellationState::Indeterminate => {
                            return Err(ChatRunStopUndecided)
                        }
                        ChatRunCancellationState::Requested
                        | ChatRunCancellationState::Finished { requested: true } => {
                            return Ok(owns_intent)
                        }
                        _ => return Ok(false),
                    }
                }
                settled.await;
            }
        })
        .await
        .map_err(|_| ChatRunStopUndecided)?
    }

    pub(crate) fn check(&self) -> Result<(), ExecutionError> {
        if self.token.is_cancelled() {
            Err(ExecutionError::Cancelled)
        } else {
            Ok(())
        }
    }

    // Inline operations cannot be recalled after they begin mutating another run or transcript.
    pub(crate) fn commit_inline(&self) -> Result<(), ExecutionError> {
        self.begin_terminal_commit()
    }

    /// Reserve the terminal outcome before its first irreversible write.
    pub(crate) fn begin_terminal_commit(&self) -> Result<(), ExecutionError> {
        self.finish(Ok(()))
    }

    pub(crate) fn begin_inline_attempt(&self) -> Result<ChatRunInlineAttempt<'_>, ExecutionError> {
        let mut state = self.state.lock().unwrap();
        if *state != ChatRunCancellationState::Open || self.token.is_cancelled() {
            return Err(ExecutionError::Cancelled);
        }
        *state = ChatRunCancellationState::Inline {
            stop_requested: false,
        };
        Ok(ChatRunInlineAttempt(self))
    }

    pub(crate) fn finish<T>(&self, result: Result<T, ExecutionError>) -> Result<T, ExecutionError> {
        let mut state = self.state.lock().unwrap();
        let requested = match *state {
            ChatRunCancellationState::Open => false,
            ChatRunCancellationState::Inline { .. } | ChatRunCancellationState::Indeterminate => {
                return result
            }
            ChatRunCancellationState::Requested => true,
            ChatRunCancellationState::ClarificationRequested(_) => {
                return Err(ExecutionError::Cancelled);
            }
            ChatRunCancellationState::Finished { requested } => requested,
        };
        *state = ChatRunCancellationState::Finished { requested };
        if requested {
            Err(ExecutionError::Cancelled)
        } else {
            result
        }
    }

    pub(crate) fn finished_after_request(&self) -> bool {
        *self.state.lock().unwrap() == ChatRunCancellationState::Finished { requested: true }
    }
}

/// Busy attempts must block cancellation during a sibling effect, but a refused attempt can retry.
pub(crate) struct ChatRunInlineAttempt<'a>(&'a ChatRunCancellation);

impl ChatRunInlineAttempt<'_> {
    pub(crate) fn complete(self) {
        let mut state = self.0.state.lock().unwrap();
        *state = ChatRunCancellationState::Finished { requested: false };
        self.0.inline_settled.notify_waiters();
    }

    /// Only call after proving that the receipt was not consumed.
    pub(crate) fn refuse(self) {
        let mut state = self.0.state.lock().unwrap();
        match *state {
            ChatRunCancellationState::Inline {
                stop_requested: true,
            } => {
                *state = ChatRunCancellationState::Requested;
                self.0.token.cancel();
            }
            ChatRunCancellationState::Inline {
                stop_requested: false,
            } => {
                *state = ChatRunCancellationState::Open;
            }
            _ => unreachable!("only an inline attempt can be refused"),
        }
        self.0.inline_settled.notify_waiters();
    }
}

impl Drop for ChatRunInlineAttempt<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        if matches!(*state, ChatRunCancellationState::Inline { .. }) {
            // The actor may have appended even when its reply never arrived.
            *state = ChatRunCancellationState::Indeterminate;
            self.0.inline_settled.notify_waiters();
        }
    }
}

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

    /// Opt in only if `execute_chat_run` consumes this handle through admission and execution.
    fn chat_run_cancellation(&self) -> Option<ChatRunCancellation> {
        None
    }

    async fn execute_chat_run(
        &self,
        _request: RunRequest,
        _agent: Arc<AgentInstance>,
        _emitter: Arc<dyn EventEmitter + Send + Sync>,
        _cancellation: ChatRunCancellation,
    ) -> Result<(), ExecutionError> {
        Err(ExecutionError::Failed(
            "chat run cancellation is not supported".into(),
        ))
    }

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

    /// Runner of isolated public telecom runs (public isolation contract §4).
    /// Default `None`: adapters without an orchestrator cannot serve public
    /// runs, and the router then refuses with a fixed notice rather than
    /// falling back to the owner path.
    fn public_harness(&self) -> Option<Arc<dyn crate::orchestrator::dispatch::HarnessRunner>> {
        None
    }

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

    /// How long `run_id` has been running, in milliseconds, measured on the
    /// server's MONOTONIC clock.
    ///
    /// A duration, deliberately, and not the wall-clock instant the run
    /// started. A client that is handed a timestamp has to answer "whose
    /// clock" before it can subtract anything, and the answer is not free:
    /// a Panel on another machine is skewed against this one by an unbounded
    /// amount, and a server clock step would move an already-reported start.
    /// A duration computed here at the instant of the answer has neither
    /// problem — the only error it carries is the one-way network delay
    /// between this line and the client rendering it.
    ///
    /// `ActiveRun::admitted_at` is the reading taken, not `started_at`:
    /// `started_at` is a wall clock and exists to be *reported*, so measuring
    /// against it would re-open on the server the exact skew this method
    /// exists to keep off the wire.
    ///
    /// `None` means "I cannot tell": the run already finished, was never held
    /// by this adapter, or this adapter has no run table at all (mocks,
    /// `SimpleExecutionEngine`). A caller must read that as "count from now",
    /// never as "it started now" — the two differ only in what they claim to
    /// know, and only the first one is true.
    async fn run_elapsed_ms(&self, _run_id: &str) -> Option<u64> {
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
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
            telecom_public: None,
            telecom_authority: None,
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
    async fn chat_run_cancellation_arbitrates_inline_commit_and_terminal_outcome() {
        let cancelled = ChatRunCancellation::default();
        assert!(cancelled.request().await.unwrap());
        assert!(cancelled.token().is_cancelled());
        assert!(matches!(
            cancelled.commit_inline(),
            Err(ExecutionError::Cancelled)
        ));
        assert!(cancelled.finished_after_request());
        assert!(!cancelled.request().await.unwrap());

        let committed = ChatRunCancellation::default();
        assert!(committed.commit_inline().is_ok());
        assert!(!committed.request().await.unwrap());
        assert!(!committed.token().is_cancelled());
        assert_eq!(
            committed.finish(Ok("inline result")).unwrap(),
            "inline result"
        );

        let completed = ChatRunCancellation::default();
        assert!(completed.finish(Ok(())).is_ok());
        assert!(!completed.request().await.unwrap());
        assert!(!completed.token().is_cancelled());

        let retry = ChatRunCancellation::default();
        let attempt = retry.begin_inline_attempt().unwrap();
        attempt.refuse();
        assert!(
            retry.request().await.unwrap(),
            "a retry still owns the original handle"
        );
        assert!(matches!(
            retry.begin_inline_attempt(),
            Err(ExecutionError::Cancelled)
        ));

        let steered = ChatRunCancellation::default();
        steered.begin_inline_attempt().unwrap().complete();
        assert!(
            !steered.request().await.unwrap(),
            "a completed fold consumed this turn"
        );
    }

    #[tokio::test]
    async fn exact_stop_awaits_inline_commit_or_refusal_without_guessing() {
        for committed in [true, false] {
            let cancellation = ChatRunCancellation::default();
            let successor = ChatRunCancellation::default();
            let append = cancellation.begin_inline_attempt().unwrap();
            let stop = cancellation.request();
            tokio::pin!(stop);
            assert!(
                futures::poll!(&mut stop).is_pending(),
                "an unresolved append cannot justify a false ACK"
            );
            assert!(
                !cancellation.token().is_cancelled(),
                "do not interrupt an append that may have committed"
            );
            if committed {
                append.complete();
            } else {
                append.refuse();
            }
            assert_eq!(stop.await.unwrap(), !committed);
            assert_eq!(cancellation.token().is_cancelled(), !committed);
            assert!(!successor.token().is_cancelled());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn exact_stop_intent_survives_a_dropped_or_expired_waiter() {
        for expired in [true, false] {
            let cancellation = ChatRunCancellation::default();
            let append = cancellation.begin_inline_attempt().unwrap();
            if expired {
                assert!(
                    cancellation.request().await.is_err(),
                    "expiry returns undecided, not false"
                );
            } else {
                let mut stop = Box::pin(cancellation.request());
                assert!(futures::poll!(&mut stop).is_pending());
                drop(stop);
            }
            append.refuse();
            assert!(matches!(
                cancellation.check(),
                Err(ExecutionError::Cancelled)
            ));
            assert!(matches!(
                cancellation.begin_inline_attempt(),
                Err(ExecutionError::Cancelled)
            ));
        }
    }

    #[tokio::test]
    async fn unknown_inline_append_never_reopens_the_receipt() {
        let cancellation = ChatRunCancellation::default();
        let append = cancellation.begin_inline_attempt().unwrap();
        let stop = cancellation.request();
        tokio::pin!(stop);
        assert!(futures::poll!(&mut stop).is_pending());
        drop(append);
        assert!(stop.await.is_err());
        assert!(cancellation.request().await.is_err());
        assert!(!cancellation.token().is_cancelled());
        assert!(cancellation.begin_inline_attempt().is_err());
    }

    #[tokio::test]
    async fn test_mock_adapter_cancel() {
        let adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter::new());

        let result = adapter.cancel("nonexistent").await;
        assert!(matches!(result, Err(ExecutionError::RunNotFound(_))));
    }
}
