//! ExecutionEngine — bridges Gateway requests to the AgentLoop.
//!
//! This file contains the struct definition, constructor, builder methods,
//! and the main `execute()` / `get_status()` / `cancel()` public API.
//!
//! Slash command handling lives in `slash_command.rs`.
//! Agent loop execution and streaming live in `run_loop.rs`.

use std::collections::HashMap;
use crate::sync_primitives::{AtomicU32, AtomicU64};
use crate::sync_primitives::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunRequest, RunState, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::gateway::agent_env::AgentEnvStore;

use crate::dispatcher::UnifiedTool;
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::run_loop::write_conversation_memory;

/// Execution engine that bridges Gateway to the AgentLoop
pub struct ExecutionEngine<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> {
    pub(super) config: ExecutionEngineConfig,
    pub(super) active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
    /// Provider registry for LLM access
    pub(super) provider_registry: Arc<P>,
    /// Tool registry for tool execution
    pub(super) tool_registry: Arc<R>,
    /// Available tools for all agents
    pub(super) tools: Arc<Vec<UnifiedTool>>,
    /// Workspace manager for workspace-scoped profile resolution
    pub(super) workspace_manager: Option<Arc<AgentEnvStore>>,
    /// Memory backend for auto-memorization of conversations
    pub(super) memory_backend: Option<crate::memory::store::MemoryBackend>,
    /// Optional task router for pre-classification and escalation handling
    pub(super) task_router: Option<Arc<dyn crate::routing::TaskRouter>>,
    /// Compression service for turn-based fact extraction
    pub(super) compression_service: Option<Arc<crate::memory::compression::CompressionService>>,
    /// Memory context provider for LanceDB-backed prompt augmentation
    pub(super) memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
    /// Global tool permission policy
    pub(super) global_tool_permissions: crate::config::types::policies::ToolPermissionsConfig,
    /// Session compactor for hierarchical session summarization
    pub(super) session_compactor: Option<Arc<crate::memory::session_compactor::SessionCompactor>>,
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Create a new execution engine
    pub fn new(
        config: ExecutionEngineConfig,
        provider_registry: Arc<P>,
        tool_registry: Arc<R>,
        tools: Vec<UnifiedTool>,
        memory_backend: Option<crate::memory::store::MemoryBackend>,
    ) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            provider_registry,
            tool_registry,
            tools: Arc::new(tools),
            workspace_manager: None,
            memory_backend,
            task_router: None,
            compression_service: None,
            memory_context_provider: None,
            global_tool_permissions: Default::default(),
            session_compactor: None,
        }
    }

    /// Set a task router for pre-classification of incoming requests.
    pub fn with_task_router(mut self, router: Arc<dyn crate::routing::TaskRouter>) -> Self {
        self.task_router = Some(router);
        self
    }

    /// Set a compression service for automatic turn-based compression.
    pub fn with_compression_service(
        mut self,
        service: Arc<crate::memory::compression::CompressionService>,
    ) -> Self {
        self.compression_service = Some(service);
        self
    }

    /// Set a memory context provider for LanceDB-backed prompt augmentation.
    pub fn with_memory_context_provider(
        mut self,
        provider: Arc<crate::thinker::MemoryContextProvider>,
    ) -> Self {
        self.memory_context_provider = Some(provider);
        self
    }

    /// Set a session compactor for hierarchical session summarization.
    pub fn with_session_compactor(
        mut self,
        compactor: Arc<crate::memory::session_compactor::SessionCompactor>,
    ) -> Self {
        self.session_compactor = Some(compactor);
        self
    }

    /// Set global tool permission policy.
    pub fn with_global_tool_permissions(
        mut self,
        permissions: crate::config::types::policies::ToolPermissionsConfig,
    ) -> Self {
        self.global_tool_permissions = permissions;
        self
    }

    /// Set the workspace manager for workspace-scoped profile resolution.
    ///
    /// When set, the engine resolves the user's active workspace at the start
    /// of each run and injects the workspace profile into the prompt builder
    /// and the workspace_id into the request context metadata.
    pub fn with_workspace_manager(mut self, manager: Arc<AgentEnvStore>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    /// Execute a run request
    ///
    /// Returns a stream of events for the run.
    ///
    /// # Arguments
    ///
    /// * `request` - The run request containing input and metadata
    /// * `agent` - The agent instance to execute with
    /// * `emitter` - Event emitter for streaming events
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        mut request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<(), ExecutionError> {
        let run_id = request.run_id.clone();

        // Create cancellation channel
        let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);

        // Atomically check concurrent run limit and register the run
        {
            let mut runs = self.active_runs.write().await;
            let agent_runs = runs
                .values()
                .filter(|r| {
                    r.request.session_key.agent_id() == request.session_key.agent_id()
                        && matches!(r.state, RunState::Running)
                })
                .count();

            if agent_runs >= self.config.max_concurrent_runs {
                return Err(ExecutionError::TooManyRuns(format!(
                    "Agent {} has {} active runs (max: {})",
                    request.session_key.agent_id(),
                    agent_runs,
                    self.config.max_concurrent_runs
                )));
            }

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

        // Check agent state (after registration to reserve the slot)
        if !agent.is_idle().await {
            // Remove the just-inserted run since agent is busy
            let mut runs = self.active_runs.write().await;
            runs.remove(&run_id);
            return Err(ExecutionError::AgentBusy(agent.id().to_string()));
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

        // Log lifecycle event: agent started
        info!(
            event_type = "agent.lifecycle.started",
            agent_id = %agent.id(),
            run_id = %run_id,
            "Agent execution started"
        );

        // Ensure session exists in memory + SQLite before adding messages
        agent.ensure_session(&request.session_key).await;

        // Store user message in session
        agent
            .add_message(&request.session_key, MessageRole::User, &request.input)
            .await;

        // ================================================================
        // Inline slash command resolution for non-router paths (Panel, CLI)
        // When input starts with / but no pre-resolved mode exists, try
        // to match against registered tools and inject the fast-path metadata.
        // ================================================================
        if request.input.trim().starts_with('/')
            && !request.metadata.contains_key(SLASH_COMMAND_MODE_KEY)
        {
            if let Some(mode_json) = self.try_resolve_slash_command(&request.input) {
                request
                    .metadata
                    .insert(SLASH_COMMAND_MODE_KEY.to_string(), mode_json);
            }
        }

        // ================================================================
        // Propagate session context BEFORE fast path so agent management
        // tools (agent_create, agent_delete) can resolve the session correctly.
        // ================================================================
        if let Some(sc_handle) = self.tool_registry.session_context_handle() {
            let mut sc = sc_handle.write().await;
            sc.channel = request.metadata.get("channel_id").cloned().unwrap_or_default();
            sc.peer_id = request.metadata.get("sender_id").cloned().unwrap_or_default();
            sc.session_key_str = request.session_key.to_key_string();
        }

        // Propagate session key to memory_search so scope=current_session works
        if let Some(sk_handle) = self.tool_registry.session_key_handle() {
            *sk_handle.write().await = request.session_key.to_key_string();
        }

        // ================================================================
        // Slash command fast path (L0): bypass full agent loop
        // ================================================================
        if let Some(mode_json) = request.metadata.get(SLASH_COMMAND_MODE_KEY) {
            let fast_result = self
                .execute_slash_command_fast_path(
                    &run_id, mode_json, &request, agent.clone(), emitter.clone(),
                )
                .await;

            match fast_result {
                Ok(response) => {
                    return self
                        .finalize_fast_path_success(&run_id, &request, &agent, &emitter, response)
                        .await;
                }
                Err(ref e) => {
                    let error_msg = e.to_string();
                    let is_skill_fallthrough = error_msg.contains("SKILL_FALLTHROUGH:");

                    if is_skill_fallthrough {
                        // Skills need LLM processing — fall through to agent loop
                        let mut runs = self.active_runs.write().await;
                        if let Some(run) = runs.get_mut(&run_id) {
                            run.state = RunState::Running;
                        }
                        warn!(
                            run_id = %run_id,
                            "Skill command falling through to agent loop"
                        );
                        // Fall through to normal agent loop
                    } else {
                        // Direct tool errors: return error response, do NOT fall through
                        return self
                            .finalize_fast_path_error(&run_id, &request, &agent, &emitter, &error_msg)
                            .await;
                    }
                }
            }
        }

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

        // Log lifecycle event: agent completed
        info!(
            event_type = "agent.lifecycle.completed",
            agent_id = %agent.id(),
            run_id = %run_id,
            success = matches!(final_state, RunState::Completed),
            "Agent execution completed"
        );

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
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;

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

                // Notify UI that the session was updated
                let _ = emitter
                    .emit(StreamEvent::SessionUpdated {
                        session_key: request.session_key.to_key_string(),
                    })
                    .await;

                // Async write to memory system (Layer 1)
                if let Some(ref mb) = self.memory_backend {
                    let mb = mb.clone();
                    let sk = request.session_key.to_key_string();
                    let agent_id = request.session_key.agent_id().to_string();
                    let ui = request.input.clone();
                    let ao = response.clone();
                    tokio::spawn(async move {
                        write_conversation_memory(mb, sk, agent_id, ui, ao).await;
                    });
                }
                // Record conversation turn for compression scheduling
                if let Some(ref cs) = self.compression_service {
                    cs.record_turn_and_check();
                }

                // Async session compaction (hierarchical summarization)
                if let Some(ref sc) = self.session_compactor {
                    let sc = sc.clone();
                    let agent_clone = agent.clone();
                    let session_key_clone = request.session_key.clone();
                    tokio::spawn(async move {
                        if let Err(e) = sc.post_turn_compress(&agent_clone, &session_key_clone).await {
                            warn!(error = %e, "Session compaction failed");
                        }
                    });
                }
                Ok(())
            }
            Err(e) => {
                // Only emit RunError for system-level errors (Timeout, Cancelled).
                // Loop failures (ExecutionError::Failed) have already emitted
                // RunError via callback, so re-emitting would cause
                // duplicate error messages on channels like Telegram.
                match e {
                    ExecutionError::Timeout | ExecutionError::Cancelled => {
                        let _ = emitter
                            .emit(StreamEvent::RunError {
                                run_id: run_id.clone(),
                                seq: final_seq,
                                error: e.to_string(),
                                error_code: Some(match e {
                                    ExecutionError::Timeout => "TIMEOUT".to_string(),
                                    ExecutionError::Cancelled => "CANCELLED".to_string(),
                                    _ => unreachable!(),
                                }),
                            })
                            .await;
                    }
                    _ => {
                        // Already reported via callback — skip duplicate emission
                    }
                }
                Err(ExecutionError::Failed(e.to_string()))
            }
        };

        // Remove from active runs after a short delay (for status queries)
        let runs_clone = active_runs.clone();
        let run_id_clone = run_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
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

    // ================================================================
    // Private helpers for fast-path finalization (DRY)
    // ================================================================

    /// Finalize a successful slash command fast-path execution.
    async fn finalize_fast_path_success<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &Arc<E>,
        response: String,
    ) -> Result<(), ExecutionError> {
        let (started_at, steps_completed, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.state = RunState::Completed;
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        agent.set_state(AgentState::Idle).await;
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;

        agent
            .add_message(&request.session_key, MessageRole::Assistant, &response)
            .await;
        let _ = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq: final_seq,
                summary: RunSummary {
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: steps_completed,
                    final_response: Some(response),
                },
                total_duration_ms: duration_ms,
            })
            .await;
        let _ = emitter
            .emit(StreamEvent::SessionUpdated {
                session_key: request.session_key.to_key_string(),
            })
            .await;

        // Remove from active runs after a short delay (same as normal path)
        let runs_clone = self.active_runs.clone();
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_owned);
        });

        Ok(())
    }

    /// Finalize a failed slash command fast-path execution (non-fallthrough error).
    async fn finalize_fast_path_error<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &Arc<E>,
        error_msg: &str,
    ) -> Result<(), ExecutionError> {
        let (started_at, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.state = RunState::Completed;
                run.cancel_tx = None;
                (run.started_at, run.next_seq())
            } else {
                (chrono::Utc::now(), 0)
            }
        };

        agent.set_state(AgentState::Idle).await;
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        let error_response = format!("\u{274c} {}", error_msg);

        agent
            .add_message(&request.session_key, MessageRole::Assistant, &error_response)
            .await;
        let _ = emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq: 1,
                content: error_response.clone(),
                chunk_index: 0,
                is_final: true,
                is_intermediate: false,
            })
            .await;
        let _ = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq: final_seq,
                summary: RunSummary {
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: 0,
                    final_response: Some(error_response),
                },
                total_duration_ms: duration_ms,
            })
            .await;
        let _ = emitter
            .emit(StreamEvent::SessionUpdated {
                session_key: request.session_key.to_key_string(),
            })
            .await;
        warn!(
            run_id = %run_id,
            error = %error_msg,
            "Slash command fast path failed, returning error to user"
        );

        // Remove from active runs after a short delay (same as normal path)
        let runs_clone = self.active_runs.clone();
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_owned);
        });

        Ok(())
    }
}
