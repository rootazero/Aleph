//! Simple Execution Engine
//!
//! Provides `SimpleExecutionEngine` for use when full provider/tool integration
//! is not available. Emits placeholder responses with simulated streaming events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunRequest, RunState, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, RunSummary, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;

/// Simple execution engine without full AgentLoop integration
/// Used when providers/tools are not available
pub struct SimpleExecutionEngine {
    config: ExecutionEngineConfig,
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
}

impl SimpleExecutionEngine {
    /// Create a new simple execution engine
    pub fn new(config: ExecutionEngineConfig) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
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

        // Check agent state
        if !agent.is_idle().await {
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

        // Set agent state to running
        agent
            .set_state(AgentState::Running {
                run_id: run_id.clone(),
            })
            .await;

        // Store user message in session
        agent
            .add_message(&request.session_key, MessageRole::User, &request.input)
            .await;

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

        // Reset agent state
        agent.set_state(AgentState::Idle).await;

        // Emit completion events
        let (started_at, steps_completed, final_seq) = {
            let runs = self.active_runs.read().await;
            if let Some(run) = runs.get(&run_id) {
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds() as u64;

        match &result {
            Ok(response) => {
                agent
                    .add_message(&request.session_key, MessageRole::Assistant, response)
                    .await;

                let _ = emitter
                    .emit(StreamEvent::RunComplete {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        summary: RunSummary {
                            total_tokens: 0,
                            tool_calls: 0,
                            loops: steps_completed,
                            final_response: Some(response.clone()),
                        },
                        total_duration_ms: duration_ms,
                    })
                    .await;
                Ok(())
            }
            Err(e) => {
                let _ = emitter
                    .emit(StreamEvent::RunError {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        error: e.to_string(),
                        error_code: Some(match e {
                            ExecutionError::Timeout => "TIMEOUT".to_string(),
                            ExecutionError::Cancelled => "CANCELLED".to_string(),
                            _ => "INTERNAL_ERROR".to_string(),
                        }),
                    })
                    .await;
                Err(ExecutionError::Failed(e.to_string()))
            }
        }
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
            runs.get(run_id).map(|r| r.next_seq()).unwrap_or(0)
        };

        let get_chunk = || async {
            let runs = self.active_runs.read().await;
            runs.get(run_id).map(|r| r.next_chunk()).unwrap_or(0)
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
                    content: chunk_str,
                    chunk_index: get_chunk().await,
                    is_final: false,
                })
                .await;
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }

        let _ = emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq: get_seq().await,
                content: String::new(),
                chunk_index: get_chunk().await,
                is_final: true,
            })
            .await;

        Ok(response)
    }

    /// Get the status of a run
    pub async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        let runs = self.active_runs.read().await;
        runs.get(run_id).map(|run| RunStatus {
            run_id: run_id.to_string(),
            state: run.state.clone(),
            started_at: Some(run.started_at),
            completed_at: match run.state {
                RunState::Completed | RunState::Cancelled | RunState::Failed { .. } => {
                    Some(chrono::Utc::now())
                }
                _ => None,
            },
            steps_completed: run.steps_completed,
            current_tool: run.current_tool.clone(),
        })
    }

    /// Cancel a run
    pub async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        let runs = self.active_runs.read().await;

        if let Some(run) = runs.get(run_id) {
            if let Some(ref cancel_tx) = run.cancel_tx {
                let _ = cancel_tx.send(()).await;
                info!("Sent cancellation signal for run {}", run_id);
                return Ok(());
            } else {
                return Err(ExecutionError::RunNotActive(run_id.to_string()));
            }
        }

        Err(ExecutionError::RunNotFound(run_id.to_string()))
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

/// Implement ExecutionAdapter for SimpleExecutionEngine.
///
/// This allows InboundMessageRouter to use SimpleExecutionEngine via a trait object,
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
        SimpleExecutionEngine::execute(self, request, agent, wrapper).await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        SimpleExecutionEngine::cancel(self, run_id).await
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        SimpleExecutionEngine::get_status(self, run_id).await
    }
}
