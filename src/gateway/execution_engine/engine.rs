//! ExecutionEngine — bridges Gateway requests to the AgentLoop.
//!
//! This file contains the struct definition, constructor, builder methods,
//! and the main `execute()` / `get_status()` / `cancel()` public API.
//!
//! Slash command handling lives in `slash_command.rs`.
//! Agent loop execution and streaming live in `run_loop.rs`.

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicU32, AtomicU64};
use std::collections::HashMap;

use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunRequest, RunState, RunStatus};
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::resilience::{AgentTask, Lane, RiskLevel, StateDatabase, TaskStatus};

use crate::dispatcher::UnifiedTool;
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::run_loop::write_conversation_memory;

#[allow(dead_code)]
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
    /// Memory context provider for SQLite-backed prompt augmentation
    pub(super) memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
    /// Global tool permission policy
    pub(super) global_tool_permissions: crate::config::types::policies::ToolPermissionsConfig,
    /// Session compactor for hierarchical session summarization
    pub(super) session_compactor: Option<Arc<crate::memory::session_compactor::SessionCompactor>>,
    /// Session manager for auto-topic generation
    pub(super) session_manager: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// Event bus for broadcasting session updates
    pub(super) event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    /// Media processor for multimodal attachment handling (images, audio, etc.)
    pub(super) media_processor: Option<Arc<crate::media::processor::MediaProcessor>>,
    /// Optional resilience database for task/trace persistence.
    pub(super) state_database: Option<Arc<StateDatabase>>,
    /// Teammate manager for named sub-agent team creation/registration.
    pub(super) teammate_manager: Option<Arc<crate::agents::teammates::TeammateManager>>,
    /// Message router for sub-agent send_message actions.
    pub(super) message_router: Option<Arc<crate::teams::messages::router::MessageRouter>>,
    /// Inbox for sub-agent read_inbox actions.
    pub(super) inbox: Option<Arc<crate::teams::messages::inbox::Inbox>>,
    /// Orchestrator handle injected after boot assembly. Populated via
    /// `with_orchestrator` once `initialize_orchestrator` completes.
    pub(super) orchestrator: Arc<std::sync::OnceLock<Arc<crate::orchestrator::Orchestrator>>>,
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
            session_manager: None,
            event_bus: None,
            media_processor: None,
            state_database: None,
            teammate_manager: None,
            message_router: None,
            inbox: None,
            orchestrator: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Return the orchestrator OnceLock handle so boot code can inject the
    /// `Arc<Orchestrator>` after `initialize_orchestrator` completes.
    pub fn orchestrator_cell(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<crate::orchestrator::Orchestrator>>> {
        self.orchestrator.clone()
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

    /// Set a memory context provider for SQLite-backed prompt augmentation.
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

    /// Set session manager and event bus for auto-topic generation.
    pub fn with_session_topic_support(
        mut self,
        session_manager: Arc<dyn crate::gateway::session_store::SessionStore>,
        event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    ) -> Self {
        self.session_manager = Some(session_manager);
        self.event_bus = Some(event_bus);
        self
    }

    /// Set the media processor for multimodal attachment handling.
    pub fn with_media_processor(
        mut self,
        processor: Arc<crate::media::processor::MediaProcessor>,
    ) -> Self {
        self.media_processor = Some(processor);
        self
    }

    /// Set the resilience state database for task/trace persistence.
    pub fn with_state_database(mut self, state_database: Arc<StateDatabase>) -> Self {
        self.state_database = Some(state_database);
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

    /// Set the teammate manager for named sub-agent team creation/registration.
    pub fn with_teammate_manager(
        mut self,
        mgr: Arc<crate::agents::teammates::TeammateManager>,
    ) -> Self {
        self.teammate_manager = Some(mgr);
        self
    }

    /// Set the message router for sub-agent send_message actions.
    pub fn with_message_router(
        mut self,
        router: Arc<crate::teams::messages::router::MessageRouter>,
    ) -> Self {
        self.message_router = Some(router);
        self
    }

    /// Set the inbox for sub-agent read_inbox actions.
    pub fn with_inbox(mut self, inbox: Arc<crate::teams::messages::inbox::Inbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    pub(super) async fn persist_run_task_started(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
    ) -> bool {
        let Some(db) = self.state_database.as_ref() else {
            return false;
        };

        let metadata_json = serde_json::to_string(&serde_json::json!({
            "run_id": run_id,
            "session_key": request.session_key.to_key_string(),
            "channel_id": request.metadata.get("channel_id"),
            "sender_id": request.metadata.get("sender_id"),
            "conversation_id": request.metadata.get("conversation_id"),
            "source": "gateway_execution_engine"
        }))
        .ok();

        let mut task = AgentTask::new(
            run_id,
            request.session_key.to_key_string(),
            agent.id().to_string(),
            request.input.clone(),
            RiskLevel::High,
        )
        .with_lane(Lane::Main);
        task.metadata_json = metadata_json;

        if let Err(error) = db.insert_agent_task(&task).await {
            warn!(
                run_id = %run_id,
                error = %error,
                "Failed to persist execution task"
            );
            return false;
        }

        if let Err(error) = db.update_task_status(run_id, TaskStatus::Running).await {
            warn!(
                run_id = %run_id,
                error = %error,
                "Failed to mark execution task as running"
            );
        }

        true
    }

    pub(super) async fn persist_run_task_status(&self, run_id: &str, status: TaskStatus) {
        let Some(db) = self.state_database.as_ref() else {
            return;
        };

        if let Err(error) = db.update_task_status(run_id, status).await {
            warn!(
                run_id = %run_id,
                status = %status,
                error = %error,
                "Failed to update execution task status"
            );
        }
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
        let (cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

        // Create CancellationToken for fine-grained agent loop cancellation.
        // The bridge task converts the coarse cancel_rx signal into a token cancellation.
        let cancel_token = CancellationToken::new();

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
                    completed_at: None,
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

        let trace_task_persisted = self
            .persist_run_task_started(&run_id, &request, &agent)
            .await;

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

        // Check if this is the first real user message (for auto-topic generation).
        // Slash commands routed via fast-path don't count — they bypass
        // add_message entirely, so history stays empty for the next real message.
        let history_empty = agent
            .get_history(&request.session_key, Some(1))
            .await
            .is_empty();
        let is_slash = request.metadata.contains_key(SLASH_COMMAND_MODE_KEY);
        let is_first_message = history_empty && !is_slash;

        if is_first_message {
            info!(
                session_key = %request.session_key.to_key_string(),
                "First message detected for session (will generate topic)"
            );
        }

        // Store user message in session (with attachment markers for history)
        let mut session_text = request.input.clone();
        for att in &request.attachments {
            let label = att.filename.as_deref().unwrap_or("file");
            if att.mime_type.starts_with("image/") {
                session_text.push_str(&format!("\n[Image attached: {}]", att.mime_type));
            } else if att.mime_type.starts_with("audio/") {
                session_text.push_str(&format!("\n[Audio attached: {}]", att.mime_type));
            } else {
                session_text.push_str(&format!("\n[Attachment: {} ({})]", label, att.mime_type));
            }
        }
        agent
            .add_message(&request.session_key, MessageRole::User, &session_text)
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
            sc.channel = request
                .metadata
                .get("channel_id")
                .cloned()
                .unwrap_or_default();
            sc.peer_id = request
                .metadata
                .get("sender_id")
                .cloned()
                .unwrap_or_default();
            sc.session_key_str = request.session_key.to_key_string();
            sc.conversation_id = request
                .metadata
                .get("conversation_id")
                .cloned()
                .unwrap_or_default();
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
                    &run_id,
                    mode_json,
                    &request,
                    agent.clone(),
                    emitter.clone(),
                )
                .await;

            match fast_result {
                Ok(response) => {
                    return self
                        .finalize_fast_path_success(&run_id, &request, &agent, &emitter, response)
                        .await;
                }
                Err(ExecutionError::Fallthrough { ref reason }) => {
                    // Skills/custom commands need LLM processing — fall through to agent loop
                    let mut runs = self.active_runs.write().await;
                    if let Some(run) = runs.get_mut(&run_id) {
                        run.state = RunState::Running;
                    }
                    warn!(
                        run_id = %run_id,
                        reason = %reason,
                        "Command falling through to agent loop"
                    );
                    // Fall through to normal agent loop
                }
                Err(ref e) => {
                    // Direct tool errors: return error response, do NOT fall through
                    return self
                        .finalize_fast_path_error(
                            &run_id,
                            &request,
                            &agent,
                            &emitter,
                            &e.to_string(),
                        )
                        .await;
                }
            }
        }

        // Execute the run
        let active_runs = self.active_runs.clone();
        let timeout_secs = request
            .timeout_secs
            .or(agent.config().timeout_secs())
            .unwrap_or(self.config.default_timeout_secs);

        let deadline = Arc::new(tokio::sync::Mutex::new(
            tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs),
        ));

        let result: Result<String, ExecutionError> = tokio::select! {
            result = self.run_agent_loop(
                &run_id,
                &request,
                agent.clone(),
                emitter.clone(),
                deadline.clone(),
                trace_task_persisted.then(|| run_id.clone()),
                cancel_token.clone(),
            ) => result,

            _ = cancel_rx.recv() => {
                // Bridge: coarse cancellation → fine-grained CancellationToken
                cancel_token.cancel();
                info!("Run {} cancelled", run_id);
                Err(ExecutionError::Cancelled)
            }

            _ = wait_for_deadline(deadline.clone()) => {
                cancel_token.cancel();
                warn!("Run {} timed out after {}s effective time", run_id, timeout_secs);
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
                run.completed_at = Some(chrono::Utc::now());
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
                self.persist_run_task_status(&run_id, TaskStatus::Completed)
                    .await;

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

                // Auto-generate session topic on first real message
                if is_first_message {
                    if let (Some(sm), Some(eb)) =
                        (self.session_manager.clone(), self.event_bus.clone())
                    {
                        let topic_provider = self
                            .provider_registry
                            .get("haiku")
                            .unwrap_or_else(|| self.provider_registry.default_provider());
                        let topic_session_key = request.session_key.clone();
                        let topic_message = request.input.clone();
                        info!(
                            session_key = %topic_session_key.to_key_string(),
                            "Auto-topic: spawning generation for first message"
                        );
                        tokio::spawn(async move {
                            use crate::providers::adapter::RequestPayload;
                            use crate::providers::message::UnifiedMessage;

                            let prompt = format!(
                                "Generate a concise topic title (5-10 characters, same language as the message) \
                                 for a conversation that starts with: {}",
                                topic_message
                            );
                            let messages = vec![UnifiedMessage::user(&prompt)];
                            let payload = RequestPayload {
                                messages: &messages,
                                system_prompt: Some("You are a title generator. Output ONLY the title, nothing else."),
                                tools: None,
                                think_level: None,
                                temperature: Some(0.3),
                                max_tokens: None,
                                tool_choice: None,
                                model: None,
                            };

                            let topic_text = match topic_provider.process(payload).await {
                                Ok(resp) => {
                                    let text = resp.text_content().trim().to_string();
                                    if text.is_empty() {
                                        None
                                    } else {
                                        Some(text)
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Auto-topic: LLM call failed, using fallback");
                                    None
                                }
                            };

                            // Fallback: use truncated first message as topic
                            let topic_text = topic_text.unwrap_or_else(|| {
                                let msg = topic_message.trim();
                                let truncated: String = msg.chars().take(20).collect();
                                if msg.chars().count() > 20 {
                                    format!("{}…", truncated)
                                } else {
                                    truncated
                                }
                            });

                            if let Err(e) = sm.set_topic(&topic_session_key, &topic_text).await {
                                warn!(error = %e, "Auto-topic: failed to persist topic");
                            } else {
                                let event_json = serde_json::json!({
                                    "method": "stream.session_updated",
                                    "params": {
                                        "session_key": topic_session_key.to_key_string(),
                                        "topic": topic_text,
                                    }
                                });
                                eb.publish(event_json.to_string());
                                info!(
                                    session_key = %topic_session_key.to_key_string(),
                                    topic = %topic_text,
                                    "Auto-topic: session topic set"
                                );
                            }
                        });
                    } else {
                        info!(
                            "Auto-topic: skipped (session_manager={}, event_bus={})",
                            self.session_manager.is_some(),
                            self.event_bus.is_some()
                        );
                    }
                }

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
                        if let Err(e) = sc
                            .post_turn_compress(&agent_clone, &session_key_clone)
                            .await
                        {
                            warn!(error = %e, "Session compaction failed");
                        }
                    });
                }
                Ok(())
            }
            Err(e) => {
                let task_status = match e {
                    ExecutionError::Cancelled => TaskStatus::Interrupted,
                    _ => TaskStatus::Failed,
                };
                self.persist_run_task_status(&run_id, task_status).await;

                let error_code = match &e {
                    ExecutionError::Timeout => "TIMEOUT",
                    ExecutionError::Cancelled => "CANCELLED",
                    ExecutionError::Failed(_) => "FAILED",
                    ExecutionError::TooManyRuns(_) => "TOO_MANY_RUNS",
                    ExecutionError::AgentBusy(_) => "AGENT_BUSY",
                    ExecutionError::RunNotFound(_) => "RUN_NOT_FOUND",
                    ExecutionError::RunNotActive(_) => "RUN_NOT_ACTIVE",
                    ExecutionError::Escalated { .. } => "ESCALATED",
                    ExecutionError::Fallthrough { .. } => "FALLTHROUGH",
                    ExecutionError::Orchestrator(_) => "ORCHESTRATOR",
                };
                let _ = emitter
                    .emit(StreamEvent::RunError {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        error: e.to_string(),
                        error_code: Some(error_code.to_string()),
                    })
                    .await;
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
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        agent.set_state(AgentState::Idle).await;
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        self.persist_run_task_status(run_id, TaskStatus::Completed)
            .await;

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
                run.state = RunState::Failed {
                    error: error_msg.to_string(),
                };
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.next_seq())
            } else {
                (chrono::Utc::now(), 0)
            }
        };

        agent.set_state(AgentState::Idle).await;
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        self.persist_run_task_status(run_id, TaskStatus::Failed)
            .await;
        let error_response = format!("\u{274c} {}", error_msg);

        agent
            .add_message(
                &request.session_key,
                MessageRole::Assistant,
                &error_response,
            )
            .await;
        let _ = emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq: 1,
                delta: error_response.clone(),
                content: error_response.clone(),
                full_text: String::new(),
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

    /// Phase 5 orchestrator-backed dispatch. Used by Task 13 integration tests
    /// and (eventually) direct Gateway callers. Returns a minimal `FlowOutcome`.
    /// Full Gateway sink streaming is NOT yet wired — events are consumed but
    /// not re-emitted. This is Phase 5's "plumbing only" landing.
    ///
    /// The orchestrator is read from the engine's `orchestrator` field, which
    /// is populated at boot by the `orchestrator_cell()` handle.
    pub async fn dispatch_via_orchestrator(
        &self,
        agent_id: String,
        input_text: String,
        session_key: String,
        channel: Option<String>,
    ) -> Result<crate::orchestrator::FlowOutcome, ExecutionError> {
        use crate::orchestrator::{FlowInput, FlowRequest, FlowStreamEvent};
        use tokio::sync::broadcast;

        let orchestrator = self
            .orchestrator
            .get()
            .ok_or_else(|| {
                ExecutionError::Orchestrator(
                    "orchestrator not yet initialised — boot ordering error".to_string(),
                )
            })?
            .clone();

        let req = FlowRequest {
            flow_id: None,
            agent_id,
            input: FlowInput::Prompt(input_text),
            channel,
            session_hint: Some(session_key),
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
        };

        let handle = orchestrator
            .dispatch(req)
            .await
            .map_err(|e| ExecutionError::Orchestrator(format!("dispatch: {e}")))?;

        // Drain events; sink wiring is Phase 6. Complete(outcome) event or
        // channel close both terminate the drain.
        let mut events = handle.events;
        loop {
            match events.recv().await {
                Ok(FlowStreamEvent::Complete(_outcome)) => break,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "orchestrator event stream lagged; dropping");
                }
            }
        }

        handle
            .completion
            .await
            .map_err(|e| ExecutionError::Orchestrator(format!("completion dropped: {e}")))?
            .map_err(|e| ExecutionError::Orchestrator(format!("flow: {e}")))
    }
}

/// Wait until the resettable deadline expires.
///
/// The deadline can be extended by compression tasks. This function re-checks
/// after waking to handle extensions that occurred during sleep.
#[allow(dead_code)]
pub(super) async fn wait_for_deadline(deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>) {
    loop {
        let dl = *deadline.lock().await;
        tokio::time::sleep_until(dl).await;
        // Re-check: deadline may have been extended while we slept.
        if tokio::time::Instant::now() >= *deadline.lock().await {
            break;
        }
        // Guard against theoretical busy-spin if deadline is in the past
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
