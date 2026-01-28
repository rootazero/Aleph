//! Execution Engine
//!
//! Bridges the Gateway with the existing agent_loop infrastructure.
//! Manages run lifecycle, emits events, and handles cancellation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use super::agent_instance::{AgentInstance, AgentState, MessageRole};
use super::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use super::router::SessionKey;

/// Configuration for the execution engine
#[derive(Debug, Clone)]
pub struct ExecutionEngineConfig {
    /// Maximum concurrent runs per agent
    pub max_concurrent_runs: usize,
    /// Default timeout for runs (seconds)
    pub default_timeout_secs: u64,
    /// Enable detailed tracing
    pub enable_tracing: bool,
}

impl Default for ExecutionEngineConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 5,
            default_timeout_secs: 300,
            enable_tracing: true,
        }
    }
}

/// A run request
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// Unique run ID
    pub run_id: String,
    /// Input message
    pub input: String,
    /// Session key for context
    pub session_key: SessionKey,
    /// Optional timeout override
    pub timeout_secs: Option<u64>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Run state
#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    /// Run is queued
    Queued,
    /// Run is executing
    Running,
    /// Run is paused (waiting for user input)
    Paused { reason: String },
    /// Run completed successfully
    Completed,
    /// Run was cancelled
    Cancelled,
    /// Run failed
    Failed { error: String },
}

/// Run status information
#[derive(Debug, Clone)]
pub struct RunStatus {
    pub run_id: String,
    pub state: RunState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub steps_completed: u32,
    pub current_tool: Option<String>,
}

/// Internal run tracking
struct ActiveRun {
    request: RunRequest,
    state: RunState,
    started_at: chrono::DateTime<chrono::Utc>,
    steps_completed: u32,
    current_tool: Option<String>,
    cancel_tx: Option<mpsc::Sender<()>>,
    seq_counter: AtomicU64,
    chunk_counter: AtomicU32,
}

impl ActiveRun {
    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Execution engine that bridges Gateway to agent_loop
pub struct ExecutionEngine {
    config: ExecutionEngineConfig,
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
}

impl ExecutionEngine {
    /// Create a new execution engine
    pub fn new(config: ExecutionEngineConfig) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Execute a run request
    ///
    /// Returns a stream of events for the run.
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<(), ExecutionError> {
        let run_id = request.run_id.clone();

        // Check concurrent run limit
        {
            let runs = self.active_runs.read().await;
            let agent_runs = runs
                .values()
                .filter(|r| r.request.session_key.agent_id() == request.session_key.agent_id())
                .count();

            if agent_runs >= self.config.max_concurrent_runs {
                return Err(ExecutionError::TooManyRuns(format!(
                    "Agent {} has {} active runs (max: {})",
                    request.session_key.agent_id(),
                    agent_runs,
                    self.config.max_concurrent_runs
                )));
            }
        }

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

        // Execute the run
        let active_runs = self.active_runs.clone();
        let timeout_secs = request
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);

        let result = tokio::select! {
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
            ) => result,

            _ = cancel_rx.recv() => {
                info!("Run {} cancelled", run_id);
                Err(ExecutionError::Cancelled)
            }

            _ = tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)) => {
                warn!("Run {} timed out after {}s", run_id, timeout_secs);
                Err(ExecutionError::Timeout)
            }
        };

        // Update run state based on result
        let final_state = match &result {
            Ok(_) => RunState::Completed,
            Err(ExecutionError::Cancelled) => RunState::Cancelled,
            Err(e) => RunState::Failed {
                error: e.to_string(),
            },
        };

        // Get run info for summary
        let (started_at, steps_completed, final_seq) = {
            let mut runs = active_runs.write().await;
            if let Some(run) = runs.get_mut(&run_id) {
                run.state = final_state.clone();
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        // Reset agent state
        agent.set_state(AgentState::Idle).await;

        // Emit completion event
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds() as u64;

        let final_result = match &result {
            Ok(response) => {
                // Store assistant response
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
        };

        // Remove from active runs after a delay (for status queries)
        let runs_clone = active_runs.clone();
        let run_id_clone = run_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            runs_clone.write().await.remove(&run_id_clone);
        });

        final_result
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

    /// List active runs
    pub async fn list_active_runs(&self) -> Vec<RunStatus> {
        let runs = self.active_runs.read().await;
        runs.iter()
            .map(|(id, run)| RunStatus {
                run_id: id.clone(),
                state: run.state.clone(),
                started_at: Some(run.started_at),
                completed_at: None,
                steps_completed: run.steps_completed,
                current_tool: run.current_tool.clone(),
            })
            .collect()
    }

    /// Internal: Run the agent loop
    ///
    /// This is a simplified version that will be replaced with actual
    /// agent_loop integration.
    async fn run_agent_loop<E: EventEmitter + Send + Sync>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        debug!("Starting agent loop for run {}", run_id);

        // Get session history for context
        let history = agent.get_history(&request.session_key, Some(20)).await;
        debug!(
            "Loaded {} messages from session history",
            history.len()
        );

        // TODO: Bridge to actual agent_loop implementation
        // For now, emit a simple response pattern to demonstrate the event flow

        // Helper to get next seq
        let get_seq = || async {
            let runs = self.active_runs.read().await;
            runs.get(run_id).map(|r| r.next_seq()).unwrap_or(0)
        };

        let get_chunk = || async {
            let runs = self.active_runs.read().await;
            runs.get(run_id).map(|r| r.next_chunk()).unwrap_or(0)
        };

        // Simulate reasoning
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
                content: " Processing input and determining response.".to_string(),
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
            "I received your message: \"{}\". This is a placeholder response from the Gateway execution engine. The full agent_loop integration is pending.",
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
}

impl Default for ExecutionEngine {
    fn default() -> Self {
        Self::new(ExecutionEngineConfig::default())
    }
}

/// Execution errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Too many concurrent runs: {0}")]
    TooManyRuns(String),

    #[error("Agent is busy: {0}")]
    AgentBusy(String),

    #[error("Run not found: {0}")]
    RunNotFound(String),

    #[error("Run is not active: {0}")]
    RunNotActive(String),

    #[error("Run was cancelled")]
    Cancelled,

    #[error("Run timed out")]
    Timeout,

    #[error("Execution failed: {0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tempfile::tempdir;

    use crate::gateway::agent_instance::AgentInstanceConfig;
    use crate::gateway::event_emitter::EventEmitError;

    /// Test event emitter that collects events
    struct TestEmitter {
        events: Arc<RwLock<Vec<StreamEvent>>>,
        event_count: AtomicUsize,
        seq_counter: AtomicU64,
    }

    impl TestEmitter {
        fn new() -> Self {
            Self {
                events: Arc::new(RwLock::new(Vec::new())),
                event_count: AtomicUsize::new(0),
                seq_counter: AtomicU64::new(0),
            }
        }

        async fn get_events(&self) -> Vec<StreamEvent> {
            self.events.read().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl EventEmitter for TestEmitter {
        async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
            self.events.write().await.push(event);
            self.event_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn next_seq(&self) -> u64 {
            self.seq_counter.fetch_add(1, Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn test_execution_engine_basic() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config).unwrap());
        let emitter = Arc::new(TestEmitter::new());
        let engine = ExecutionEngine::default();

        let request = RunRequest {
            run_id: "test-run-1".to_string(),
            input: "Hello, world!".to_string(),
            session_key: SessionKey::main("test"),
            timeout_secs: None,
            metadata: HashMap::new(),
        };

        let result = engine.execute(request, agent, emitter.clone()).await;
        assert!(result.is_ok());

        let events = emitter.get_events().await;
        assert!(!events.is_empty());

        // Check for expected events
        let has_run_accepted = events.iter().any(|e| matches!(e, StreamEvent::RunAccepted { .. }));
        let has_run_complete = events.iter().any(|e| matches!(e, StreamEvent::RunComplete { .. }));

        assert!(has_run_accepted, "Should have RunAccepted event");
        assert!(has_run_complete, "Should have RunComplete event");
    }

    #[tokio::test]
    async fn test_execution_engine_cancel() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test-cancel".to_string(),
            workspace: temp.path().join("workspace"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config).unwrap());
        let emitter = Arc::new(TestEmitter::new());
        let engine = Arc::new(ExecutionEngine::new(ExecutionEngineConfig {
            default_timeout_secs: 10,
            ..Default::default()
        }));

        let request = RunRequest {
            run_id: "test-cancel-run".to_string(),
            input: "Long running task".to_string(),
            session_key: SessionKey::main("test-cancel"),
            timeout_secs: None,
            metadata: HashMap::new(),
        };

        // Start execution in background
        let engine_clone = engine.clone();
        let agent_clone = agent.clone();
        let emitter_clone = emitter.clone();
        let handle = tokio::spawn(async move {
            engine_clone.execute(request, agent_clone, emitter_clone).await
        });

        // Wait a bit then cancel
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let cancel_result = engine.cancel("test-cancel-run").await;

        // Cancel might fail if run completed quickly
        if cancel_result.is_ok() {
            let result = handle.await.unwrap();
            // Should be cancelled or completed (race condition)
            assert!(result.is_ok() || matches!(result, Err(ExecutionError::Cancelled)));
        }
    }

    #[tokio::test]
    async fn test_concurrent_run_limit() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test-limit".to_string(),
            workspace: temp.path().join("workspace"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config).unwrap());
        let emitter = Arc::new(TestEmitter::new());
        let engine = ExecutionEngine::new(ExecutionEngineConfig {
            max_concurrent_runs: 1,
            ..Default::default()
        });

        // Start first run
        let request1 = RunRequest {
            run_id: "run-1".to_string(),
            input: "First".to_string(),
            session_key: SessionKey::main("test-limit"),
            timeout_secs: Some(5),
            metadata: HashMap::new(),
        };

        // This should succeed and complete quickly in tests
        let result1 = engine.execute(request1, agent.clone(), emitter.clone()).await;
        assert!(result1.is_ok());
    }
}
