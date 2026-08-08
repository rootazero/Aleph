//! Simple Execution Engine
//!
//! Provides `SimpleExecutionEngine` for use when full provider/tool integration
//! is not available. Emits placeholder responses with simulated streaming events.

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicU32, AtomicU64};
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunRequest, RunState, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, AgentState};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, RunSummary, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;

/// Simple execution engine without full `AgentLoop` integration
/// Used when providers/tools are not available
pub struct SimpleExecutionEngine {
    config: ExecutionEngineConfig,
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl SimpleExecutionEngine {
    /// Create a new simple execution engine
    #[must_use]
    pub fn new(config: ExecutionEngineConfig) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            event_bus: None,
        }
    }

    /// Attach the global event bus so session updates reach UI clients
    /// (mirrors `ExecutionEngine::publish_session_updated`).
    #[must_use]
    pub fn with_event_bus(
        mut self,
        event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    ) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Announce a session update on the global event bus. No-op without a bus.
    /// Same contract as `ExecutionEngine::publish_session_updated` (engine.rs),
    /// including the required `origin_run_id`.
    fn publish_session_updated(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        origin_channel: Option<&str>,
        origin_run_id: &str,
    ) {
        if let Some(bus) = &self.event_bus {
            let frame = crate::gateway::events::GatewayEventFrame::SessionUpdated {
                session_key: session_key.to_key_string(),
                origin_channel: origin_channel
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string()),
                origin_run_id: (!origin_run_id.is_empty()).then(|| origin_run_id.to_string()),
            };
            let _ = bus.publish_frame(&frame);
        }
    }

    /// Execute a run request with simulated response
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<(), ExecutionError> {
        let run_id = request.run_id.clone();

        // Ensure the session row exists before any state transitions; tests and
        // fallback paths may run without the global SessionService initialized.
        // Scoped for the same reason the full engine scopes it — see
        // `run_loop::ensure_session_under_request_scope`.
        super::run_loop::ensure_session_under_request_scope(&agent, &request).await;

        // Atomically check Idle and transition to Running to close the TOCTOU
        // window between an is_idle() probe and the later set_state(Running).
        if !agent.try_start_run(&run_id).await {
            return Err(ExecutionError::AgentBusy(agent.id().to_string()));
        }

        // Create cancellation channel
        let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);

        // Register the run
        {
            let mut runs = self.active_runs.write().await;
            runs.insert(
                run_id.clone(),
                ActiveRun {
                    request: request.clone(),
                    state: RunState::Running,
                    started_at: chrono::Utc::now(),
                    completed_at: None,
                    steps_completed: 0,
                    current_tool: None,
                    cancel_tx: Some(cancel_tx),
                    seq_counter: AtomicU64::new(0),
                    chunk_counter: AtomicU32::new(0),
                },
            );
        }

        // Emit run accepted event
        let _ = emitter
            .emit(StreamEvent::RunAccepted {
                run_id: run_id.clone(),
                session_key: request.session_key.to_key_string(),
                accepted_at: chrono::Utc::now().to_rfc3339(),
            })
            .await;

        // Detect a brand-new session before persisting the first message so we
        // can announce it immediately (see the emit below).
        let is_first_message = agent
            .get_history(&request.session_key, Some(1))
            .await
            .is_empty();

        // Emit user message via SessionService (Task 7a — replaces direct
        // messages-table write so MessageProjector is the single writer).
        let turn_id = uuid::Uuid::new_v4();
        if let Some(svc) = crate::session::service::global_session_service() {
            let _ = svc
                .emit_event(
                    &request.session_key,
                    crate::session::events::SessionEvent::UserMessage {
                        turn_id,
                        content: crate::session::events::MessageContent {
                            text: request.input.clone(),
                            blocks: Vec::new(),
                            thinking: None,
                            thinking_signature: None,
                        },
                        at: crate::session::events::now_ms(),
                        synthetic: false,
                        author_user_id: crate::scope::room_author_from_metadata(&request.metadata),
                    },
                )
                .await;
        }

        // Announce the session the moment it's created (first message). Mirrors
        // the full ExecutionEngine path so clients that refresh their session
        // list on `SessionUpdated` (e.g. the Panel sidebar) show the new session
        // right away instead of only after the turn completes / a manual reload.
        if is_first_message {
            self.publish_session_updated(
                &request.session_key,
                request.metadata.get("channel_id").map(String::as_str),
                &request.run_id,
            );
        }

        // Execute with timeout
        let timeout_secs = request
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);

        let result = tokio::select! {
            result = self.run_simple_loop(&run_id, &request, emitter.clone()) => result,
            _ = cancel_rx.recv() => {
                info!("Run {} cancelled", run_id);
                Err(ExecutionError::Cancelled)
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)) => {
                warn!("Run {} timed out after {}s", run_id, timeout_secs);
                Err(ExecutionError::Timeout)
            }
        };

        // Reset agent state so the agent can accept the next request.
        agent.set_state(AgentState::Idle).await;

        // Update run state and emit completion events
        let final_state = match &result {
            Ok(_) => RunState::Completed,
            Err(ExecutionError::Cancelled) => RunState::Cancelled,
            Err(e) => RunState::Failed {
                error: e.to_string(),
            },
        };
        let (started_at, steps_completed, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(&run_id) {
                run.state = final_state;
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;

        let final_result = match result {
            Ok(response) => {
                let response = &response;
                // Emit assistant message via SessionService (Task 7a — same
                // turn_id as the UserMessage above, mirrors one logical turn).
                if let Some(svc) = crate::session::service::global_session_service() {
                    let _ = svc
                        .emit_event(
                            &request.session_key,
                            crate::session::events::SessionEvent::AssistantMessage {
                                turn_id,
                                content: crate::session::events::MessageContent {
                                    text: (*response).clone(),
                                    blocks: Vec::new(),
                                    thinking: None,
                                    thinking_signature: None,
                                },
                                usage: None,
                                at: crate::session::events::now_ms(),
                            },
                        )
                        .await;
                }

                let _ = emitter
                    .emit(StreamEvent::RunComplete {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        summary: RunSummary {
                            // 0 is correct: SimpleExecutionEngine is the
                            // simulated/fallback engine used when no API key
                            // is set — no real LLM call is made.
                            total_tokens: 0,
                            tool_calls: 0,
                            loops: steps_completed,
                            final_response: Some(response.clone()),
                            ..Default::default()
                        },
                        total_duration_ms: duration_ms,
                    })
                    .await;
                Ok(())
            }
            Err(e) => {
                let error_code = match &e {
                    ExecutionError::Timeout => "TIMEOUT".to_string(),
                    ExecutionError::Cancelled => "CANCELLED".to_string(),
                    _ => "INTERNAL_ERROR".to_string(),
                };
                let _ = emitter
                    .emit(StreamEvent::RunError {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        error: e.to_string(),
                        error_code: Some(error_code),
                    })
                    .await;
                // Preserve the typed variant — see execute.rs for rationale.
                Err(e)
            }
        };

        // Clear the session-level "running" marker now that the final message
        // (or error receipt) has been persisted.
        agent.set_session_idle(&request.session_key).await;

        // Remove from active runs after a short delay (for status queries)
        let runs_clone = self.active_runs.clone();
        let run_id_clone = run_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_clone);
        });

        final_result
    }

    /// Simple loop that emits placeholder response
    async fn run_simple_loop<E: EventEmitter + Send + Sync>(
        &self,
        run_id: &str,
        request: &RunRequest,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        // Helper to get next seq
        let get_seq = || async {
            let runs = self.active_runs.read().await;
            runs.get(run_id).map_or(0, |r| r.next_seq())
        };

        let get_chunk = || async {
            let runs = self.active_runs.read().await;
            runs.get(run_id).map_or(0, |r| r.next_chunk())
        };

        // Emit reasoning
        let _ = emitter
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq: get_seq().await,
                content: "Analyzing the request...".to_string(),
                is_complete: false,
            })
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let _ = emitter
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq: get_seq().await,
                content: " Processing complete.".to_string(),
                is_complete: true,
            })
            .await;

        // Update step count
        {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.steps_completed += 1;
            }
        }

        // Generate response
        let response = format!(
            "I received your message: \"{}\". This response is from the Gateway's simple execution engine. For full agent capabilities, configure a provider.",
            request.input
        );

        // Emit response chunks
        for chunk in response.chars().collect::<Vec<_>>().chunks(50) {
            let chunk_str: String = chunk.iter().collect();
            let _ = emitter
                .emit(StreamEvent::ResponseChunk {
                    run_id: run_id.to_string(),
                    seq: get_seq().await,
                    delta: chunk_str,
                    full_text: String::new(),
                    chunk_index: get_chunk().await,
                    is_final: false,
                    is_intermediate: false,
                })
                .await;
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }

        let _ = emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq: get_seq().await,
                delta: String::new(),
                full_text: String::new(),
                chunk_index: get_chunk().await,
                is_final: true,
                is_intermediate: false,
            })
            .await;

        Ok(response)
    }

    /// Get the number of currently active (non-completed) runs
    pub async fn active_run_count(&self) -> usize {
        self.active_runs.read().await.len()
    }

    /// Get the status of a run
    pub async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        let runs = self.active_runs.read().await;
        runs.get(run_id).map(|run| RunStatus {
            run_id: run_id.to_string(),
            state: run.state.clone(),
            started_at: Some(run.started_at),
            completed_at: run.completed_at,
            steps_completed: run.steps_completed,
            current_tool: run.current_tool.clone(),
        })
    }

    /// Cancel a run
    pub async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        let cancel_tx = {
            let runs = self.active_runs.read().await;
            match runs.get(run_id) {
                Some(run) => match run.cancel_tx {
                    Some(ref tx) => tx.clone(),
                    None => return Err(ExecutionError::RunNotActive(run_id.to_string())),
                },
                None => return Err(ExecutionError::RunNotFound(run_id.to_string())),
            }
        };
        // Lock released before await
        let _ = cancel_tx.send(()).await;
        info!("Sent cancellation signal for run {}", run_id);
        Ok(())
    }

    /// Cancel the `Running` run on `session_key`, if any. Mirrors
    /// [`super::engine::ExecutionEngine::cancel_session`] so `/stop` works
    /// against the provider-less fallback engine too.
    pub async fn cancel_session(
        &self,
        session_key: &crate::routing::session_key::SessionKey,
    ) -> Result<Option<String>, ExecutionError> {
        let target = {
            let runs = self.active_runs.read().await;
            super::steering::find_steering_target_id(&runs, "", session_key)
        };
        match target {
            Some(run_id) => {
                self.cancel(&run_id).await?;
                Ok(Some(run_id))
            }
            None => Ok(None),
        }
    }
}

impl Default for SimpleExecutionEngine {
    fn default() -> Self {
        Self::new(ExecutionEngineConfig::default())
    }
}

// ============================================================================
// ExecutionAdapter trait implementation
// ============================================================================

/// Implement `ExecutionAdapter` for `SimpleExecutionEngine`.
///
/// This allows `InboundMessageRouter` to use `SimpleExecutionEngine` via a trait object,
/// which is useful when providers/tools are not configured.
#[async_trait]
impl ExecutionAdapter for SimpleExecutionEngine {
    async fn execute(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        // Wrap the dyn trait object in DynEventEmitter to make it Sized,
        // then delegate to the existing generic execute method
        let wrapper = Arc::new(DynEventEmitter::new(emitter));
        Self::execute(self, request, agent, wrapper).await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        Self::cancel(self, run_id).await
    }

    async fn cancel_session(
        &self,
        session_key: &crate::routing::session_key::SessionKey,
    ) -> Result<Option<String>, ExecutionError> {
        Self::cancel_session(self, session_key).await
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        Self::get_status(self, run_id).await
    }

    async fn active_run_count(&self) -> usize {
        Self::active_run_count(self).await
    }

    fn concurrency_snapshot(&self) -> super::ConcurrencySnapshot {
        // SimpleExecutionEngine uses a per-agent try_start_run gate, not the
        // run-lifetime ConcurrencyLimiter. Return an explicit zeroed snapshot so
        // metrics don't read as a plausible "0 of N slots" from a real limiter.
        super::ConcurrencySnapshot::default()
    }

    fn running_sessions(&self) -> Vec<String> {
        // SimpleExecutionEngine has no per-session run registry; honest empty.
        Vec::new()
    }
}
