//! Execution Engine
//!
//! Bridges the Gateway with the existing agent_loop infrastructure.
//! Manages run lifecycle, emits events, and handles cancellation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{debug, error, info, warn};

use super::agent_instance::{AgentInstance, AgentState, MessageRole};
use super::event_emitter::{DynEventEmitter, EventEmitter, RunSummary, StreamEvent};
use super::execution_adapter::ExecutionAdapter;
use super::loop_callback_adapter::EventEmittingCallback;
use super::router::SessionKey;

use crate::agent_loop::{AgentLoop, LoopConfig, LoopResult, RequestContext};
use crate::compressor::NoOpCompressor;
use crate::dispatcher::UnifiedTool;
use crate::executor::{SingleStepExecutor, ToolRegistry};
use crate::thinker::{ProviderRegistry as ThinkerProviderRegistry, SingleProviderRegistry, Thinker, ThinkerConfig};

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
pub struct ExecutionEngine<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> {
    config: ExecutionEngineConfig,
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
    /// Abort signal senders for each run
    abort_senders: Arc<RwLock<HashMap<String, watch::Sender<bool>>>>,
    /// Provider registry for LLM access
    provider_registry: Arc<P>,
    /// Tool registry for tool execution
    tool_registry: Arc<R>,
    /// Available tools for all agents
    tools: Arc<Vec<UnifiedTool>>,
}

/// Simple execution engine without full AgentLoop integration
/// Used when providers/tools are not available
pub struct SimpleExecutionEngine {
    config: ExecutionEngineConfig,
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Create a new execution engine with full AgentLoop integration
    pub fn new(
        config: ExecutionEngineConfig,
        provider_registry: Arc<P>,
        tool_registry: Arc<R>,
        tools: Vec<UnifiedTool>,
    ) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            abort_senders: Arc::new(RwLock::new(HashMap::new())),
            provider_registry,
            tool_registry,
            tools: Arc::new(tools),
        }
    }

    /// Store abort sender for a run
    async fn store_abort_sender(&self, run_id: &str, sender: watch::Sender<bool>) {
        let mut senders = self.abort_senders.write().await;
        senders.insert(run_id.to_string(), sender);
    }

    /// Remove and trigger abort for a run
    #[allow(dead_code)]
    async fn trigger_abort(&self, run_id: &str) -> bool {
        let mut senders = self.abort_senders.write().await;
        if let Some(sender) = senders.remove(run_id) {
            let _ = sender.send(true);
            return true;
        }
        false
    }

    /// Format history for AgentLoop
    fn format_history(&self, history: &[super::agent_instance::SessionMessage]) -> String {
        let mut formatted = String::new();
        for msg in history {
            let role = match msg.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::System => "System",
                MessageRole::Tool => "Tool",
            };
            formatted.push_str(&format!("{}: {}\n", role, msg.content));
        }
        formatted
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

    /// Internal: Run the agent loop with full integration
    ///
    /// This method bridges to the actual AgentLoop infrastructure using:
    /// - Thinker for LLM decision making
    /// - SingleStepExecutor for tool execution
    /// - EventEmittingCallback for streaming events
    async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
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

        // Create abort signal channel
        let (abort_tx, abort_rx) = watch::channel(false);
        self.store_abort_sender(run_id, abort_tx).await;

        // Create callback adapter that emits StreamEvents
        let callback = Arc::new(EventEmittingCallback::new(
            emitter.clone(),
            run_id.to_string(),
        ));

        // Create Thinker with provider
        let provider = self.provider_registry.default_provider();
        let thinker_registry = Arc::new(SingleProviderRegistry::new(provider));
        let thinker = Arc::new(Thinker::new(thinker_registry, ThinkerConfig::default()));

        // Create Executor
        let executor = Arc::new(SingleStepExecutor::new(self.tool_registry.clone()));

        // Create compressor (no-op for Gateway mode)
        let compressor = Arc::new(NoOpCompressor);

        // Create AgentLoop with config
        let max_loops = agent.config().max_loops as usize;
        let loop_config = LoopConfig::default().with_max_steps(max_loops);

        // Build request context (empty for Gateway mode, context is from session history)
        let context = RequestContext::empty();

        // Format history as initial summary
        let history_summary = self.format_history(&history);

        // Filter tools based on agent whitelist/blacklist
        let allowed_tools: Vec<UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();

        // Create AgentLoop with Thinker, Executor, Compressor
        let agent_loop = AgentLoop::new(thinker, executor, compressor, loop_config);

        // Run the loop (history_summary as Option<String>)
        let initial_history = if history_summary.is_empty() {
            None
        } else {
            Some(history_summary)
        };

        let result = agent_loop
            .run(
                request.input.clone(),
                context,
                allowed_tools,
                callback.as_ref(),
                Some(abort_rx),
                initial_history,
            )
            .await;

        // Clean up abort sender
        {
            let mut senders = self.abort_senders.write().await;
            senders.remove(run_id);
        }

        // Update step count from result
        let steps = match &result {
            LoopResult::Completed { steps, .. } => *steps,
            LoopResult::Failed { steps, .. } => *steps,
            LoopResult::GuardTriggered(_) => 0,
            LoopResult::UserAborted => 0,
        };

        {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.steps_completed = steps as u32;
            }
        }

        // Convert LoopResult to response
        match result {
            LoopResult::Completed { summary, .. } => {
                info!(run_id = %run_id, "Agent loop completed successfully");
                Ok(summary)
            }
            LoopResult::Failed { reason, .. } => {
                error!(run_id = %run_id, reason = %reason, "Agent loop failed");
                Err(ExecutionError::Failed(reason))
            }
            LoopResult::GuardTriggered(violation) => {
                warn!(run_id = %run_id, violation = ?violation, "Guard triggered");
                Err(ExecutionError::Failed(violation.description()))
            }
            LoopResult::UserAborted => {
                info!(run_id = %run_id, "Agent loop aborted by user");
                Err(ExecutionError::Cancelled)
            }
        }
    }
}

// Implement SimpleExecutionEngine for when full integration is not available
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
// ExecutionAdapter trait implementations
// ============================================================================

/// Implement ExecutionAdapter for the full ExecutionEngine with AgentLoop integration.
///
/// This allows InboundMessageRouter to use ExecutionEngine via a trait object,
/// enabling routing without being generic over provider and tool registry types.
#[async_trait]
impl<P, R> ExecutionAdapter for ExecutionEngine<P, R>
where
    P: ThinkerProviderRegistry + Send + Sync + 'static,
    R: ToolRegistry + Send + Sync + 'static,
{
    async fn execute(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        // Wrap the dyn trait object in DynEventEmitter to make it Sized,
        // then delegate to the existing generic execute method
        let wrapper = Arc::new(DynEventEmitter::new(emitter));
        ExecutionEngine::execute(self, request, agent, wrapper).await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        ExecutionEngine::cancel(self, run_id).await
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        ExecutionEngine::get_status(self, run_id).await
    }
}

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
    async fn test_simple_execution_engine_basic() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test".to_string(),
            workspace: temp.path().join("workspace"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config).unwrap());
        let emitter = Arc::new(TestEmitter::new());
        let engine = SimpleExecutionEngine::default();

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
    async fn test_simple_execution_engine_run() {
        let temp = tempdir().unwrap();
        let config = AgentInstanceConfig {
            agent_id: "test-simple".to_string(),
            workspace: temp.path().join("workspace"),
            ..Default::default()
        };

        let agent = Arc::new(AgentInstance::new(config).unwrap());
        let emitter = Arc::new(TestEmitter::new());
        let engine = SimpleExecutionEngine::new(ExecutionEngineConfig {
            default_timeout_secs: 10,
            ..Default::default()
        });

        let request = RunRequest {
            run_id: "run-simple".to_string(),
            input: "Test input".to_string(),
            session_key: SessionKey::main("test-simple"),
            timeout_secs: Some(5),
            metadata: HashMap::new(),
        };

        // This should succeed and complete quickly
        let result = engine.execute(request, agent.clone(), emitter.clone()).await;
        assert!(result.is_ok());

        // Verify events were emitted
        let events = emitter.get_events().await;
        let has_reasoning = events.iter().any(|e| matches!(e, StreamEvent::Reasoning { .. }));
        let has_response = events.iter().any(|e| matches!(e, StreamEvent::ResponseChunk { .. }));

        assert!(has_reasoning, "Should have Reasoning event");
        assert!(has_response, "Should have ResponseChunk event");
    }
}
