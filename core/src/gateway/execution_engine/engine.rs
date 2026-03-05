//! Full ExecutionEngine with AgentLoop integration
//!
//! Provides the `ExecutionEngine<P, R>` struct that bridges Gateway requests
//! to the actual agent loop infrastructure using Thinker, Executor, and tools.

use std::collections::HashMap;
use crate::sync_primitives::{AtomicU32, AtomicU64};
use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{debug, error, info, warn};

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunRequest, RunState, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, RunSummary, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::loop_callback_adapter::EventEmittingCallback;
use crate::gateway::workspace::{ActiveWorkspace, WorkspaceManager};

use crate::agent_loop::{AgentLoop, LoopConfig, LoopResult, RequestContext, RunContext};
use crate::compressor::NoOpCompressor;
use crate::dispatcher::UnifiedTool;
use crate::executor::{SingleStepExecutor, ToolRegistry};
use crate::gateway::handlers::plugins::get_extension_manager;
use crate::thinker::{
    PromptConfig, ProviderRegistry as ThinkerProviderRegistry, SingleProviderRegistry, Thinker,
    ThinkerConfig,
};
use aleph_protocol::IdentityContext;

/// Execution engine that bridges Gateway to agent_loop
pub struct ExecutionEngine<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> {
    config: ExecutionEngineConfig,
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,
    /// Provider registry for LLM access
    provider_registry: Arc<P>,
    /// Tool registry for tool execution
    tool_registry: Arc<R>,
    /// Available tools for all agents
    tools: Arc<Vec<UnifiedTool>>,
    /// Session manager for identity context
    session_manager: Arc<crate::gateway::SessionManager>,
    /// Workspace manager for workspace-scoped profile resolution
    workspace_manager: Option<Arc<WorkspaceManager>>,
    /// Memory backend for auto-memorization of conversations
    memory_backend: Option<crate::memory::store::MemoryBackend>,
    /// Workspace file loader for agent-scoped SOUL.md/AGENTS.md/MEMORY.md
    workspace_loader: std::sync::Mutex<crate::gateway::workspace_loader::WorkspaceFileLoader>,
    /// Optional task router for pre-classification and escalation handling
    task_router: Option<Arc<dyn crate::routing::TaskRouter>>,
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Create a new execution engine with full AgentLoop integration
    pub fn new(
        config: ExecutionEngineConfig,
        provider_registry: Arc<P>,
        tool_registry: Arc<R>,
        tools: Vec<UnifiedTool>,
        session_manager: Arc<crate::gateway::SessionManager>,
        memory_backend: Option<crate::memory::store::MemoryBackend>,
    ) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            provider_registry,
            tool_registry,
            tools: Arc::new(tools),
            session_manager,
            workspace_manager: None,
            memory_backend,
            workspace_loader: std::sync::Mutex::new(
                crate::gateway::workspace_loader::WorkspaceFileLoader::new(),
            ),
            task_router: None,
        }
    }

    /// Set a task router for pre-classification of incoming requests.
    pub fn with_task_router(mut self, router: Arc<dyn crate::routing::TaskRouter>) -> Self {
        self.task_router = Some(router);
        self
    }

    /// Set the workspace manager for workspace-scoped profile resolution.
    ///
    /// When set, the engine resolves the user's active workspace at the start
    /// of each run and injects the workspace profile into the Thinker and
    /// the workspace_id into the request context metadata.
    pub fn with_workspace_manager(mut self, manager: Arc<WorkspaceManager>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    /// Format history for AgentLoop
    fn format_history(&self, history: &[crate::gateway::agent_instance::SessionMessage]) -> String {
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
    ///
    /// # Arguments
    ///
    /// * `request` - The run request containing input and metadata
    /// * `agent` - The agent instance to execute with
    /// * `emitter` - Event emitter for streaming events
    pub async fn execute<E: EventEmitter + Send + Sync + 'static>(
        &self,
        request: RunRequest,
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
                    let ui = request.input.clone();
                    let ao = response.clone();
                    let _ = tokio::spawn(async move {
                        write_conversation_memory(mb, sk, ui, ao).await;
                    });
                }
                Ok(())
            }
            Err(e) => {
                // Only emit RunError for system-level errors (Timeout, Cancelled).
                // AgentLoop failures (ExecutionError::Failed) have already emitted
                // RunError via callback.on_failed(), so re-emitting would cause
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
    ///
    /// # Arguments
    ///
    /// * `run_id` - Unique identifier for this run
    /// * `request` - The run request
    /// * `agent` - Agent instance
    /// * `emitter` - Event emitter for streaming
    async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        debug!(
            run_id = run_id,
            "Starting agent loop"
        );

        // Get session history for context
        let history = agent.get_history(&request.session_key, Some(20)).await;
        debug!(
            "Loaded {} messages from session history",
            history.len()
        );

        // Create abort signal channel
        let (_abort_tx, abort_rx) = watch::channel(false);

        // Create callback adapter that emits StreamEvents
        let callback = Arc::new(EventEmittingCallback::new(
            emitter.clone(),
            run_id.to_string(),
        ));

        // Create Thinker with provider
        let provider = self.provider_registry.default_provider();
        let thinker_registry = Arc::new(SingleProviderRegistry::new(provider));

        // Inject skill instructions from SkillSystem v2 snapshot into prompt
        let skill_instructions = match get_extension_manager().ok().and_then(|m| m.skill_system()) {
            Some(sys) => {
                let snapshot = sys.current_snapshot().await;
                if snapshot.prompt_xml.is_empty() { None } else { Some(snapshot.prompt_xml) }
            }
            None => None,
        };
        // Load runtime capabilities from the ledger for prompt injection
        let runtime_capabilities = {
            use crate::runtimes::ledger::CapabilityLedger;
            use crate::runtimes::format_entries_for_prompt;

            crate::utils::paths::get_runtimes_dir().ok().and_then(|dir| {
                let ledger = CapabilityLedger::load_or_create(dir.join("ledger.json"));
                let ready = ledger.list_ready();
                if ready.is_empty() { None } else { Some(format_entries_for_prompt(&ready)) }
            })
        };

        // --- Workspace Resolution ---
        // Priority: route binding workspace > user active workspace > global
        let route_workspace = request.metadata.get("route_workspace").cloned();
        let active_workspace = if let Some(ref ws_manager) = self.workspace_manager {
            if let Some(ref route_ws) = route_workspace {
                // Channel routing specifies workspace
                debug!(run_id = run_id, route_workspace = %route_ws, "Using route-resolved workspace");
                ActiveWorkspace::from_workspace_id(ws_manager, route_ws).await
            } else {
                // Use user's active workspace
                ActiveWorkspace::from_manager(ws_manager, "owner").await
            }
        } else {
            ActiveWorkspace::default_global()
        };

        // Override memory filter with agent-specific scoping for non-main agents
        let active_workspace = {
            let mut ws = active_workspace;
            let agent_id = request.session_key.agent_id();
            if agent_id != "main" {
                ws.memory_filter = crate::memory::workspace::WorkspaceFilter::Single(
                    agent_id.to_string(),
                );
            }
            ws
        };

        debug!(
            run_id = run_id,
            workspace_id = %active_workspace.workspace_id,
            profile_model = ?active_workspace.profile.model,
            "Resolved active workspace"
        );

        // Propagate workspace_id to workspace-aware tools (memory_search, memory_browse)
        if let Some(ws_handle) = self.tool_registry.workspace_handle() {
            let mut ws = ws_handle.write().await;
            *ws = active_workspace.workspace_id.clone();
            debug!(
                run_id = run_id,
                workspace_id = %active_workspace.workspace_id,
                "Updated tool workspace handle"
            );
        }

        // Propagate SmartRecallConfig from workspace profile to memory_search tool
        if let Some(sr_handle) = self.tool_registry.smart_recall_config_handle() {
            let mut sr = sr_handle.write().await;
            *sr = active_workspace.profile.smart_recall.clone();
            debug!(
                run_id = run_id,
                enabled = active_workspace.profile.smart_recall.as_ref().map_or(false, |c| c.enabled),
                "Updated tool smart recall config"
            );
        }

        // --- Identity / Bootstrap / User Profile ---
        // Resolve AI identity from ~/.aleph/soul.md (layered: session > project > global > default)
        let mut identity_resolver = crate::thinker::identity::IdentityResolver::with_defaults();
        let agent_id = request.session_key.agent_id().to_string();
        identity_resolver.add_project(&agent_id);
        let resolved_soul = identity_resolver.resolve();

        // Check bootstrap state — if no soul.md, inject First Contact Protocol
        let bootstrap_detector = crate::agent_loop::bootstrap::BootstrapDetector::new(
            identity_resolver.global_path().clone(),
        );
        let bootstrap_prompt = bootstrap_detector.bootstrap_prompt();

        info!(
            run_id = run_id,
            bootstrap = bootstrap_prompt.is_some(),
            soul_exists = !resolved_soul.is_empty(),
            "Identity/Bootstrap check"
        );

        // Load user profile from ~/.aleph/user_profile.md
        let user_profile = {
            let profile_path = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".aleph")
                .join("user_profile.md");
            crate::thinker::user_profile::UserProfile::load_from_file(&profile_path)
        };

        // Load scratchpad for current agent/project
        let scratchpad_content = {
            let manager = crate::memory::scratchpad::ScratchpadManager::new(&agent_id, run_id);
            if manager.exists() {
                manager.read().await.ok()
            } else {
                None
            }
        };

        // Build custom_instructions from bootstrap + user_profile + scratchpad
        let mut extra_instructions = Vec::new();
        if let Some(bp) = &bootstrap_prompt {
            extra_instructions.push(bp.clone());
        }
        if let Some(ref profile) = user_profile {
            if !profile.is_empty() {
                extra_instructions.push(profile.to_prompt_section());
            }
        }
        if let Some(ref pad) = scratchpad_content {
            extra_instructions.push(format!("## Active Scratchpad\n\n{}", pad));
        }
        // Inject workspace profile system_prompt if configured
        if let Some(ref ws_prompt) = active_workspace.profile.system_prompt {
            if !ws_prompt.is_empty() {
                extra_instructions.push(format!("## Workspace Profile\n\n{}", ws_prompt));
            }
        }
        // Inject workspace files from agent's workspace directory
        let agent_workspace_dir = agent.config().workspace.clone();
        let workspace_soul_candidate = {
            let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());

            // Inject AGENTS.md as agent-specific instructions
            if let Some(agents_md) = loader.load_agents_md(&agent_workspace_dir) {
                if !agents_md.is_empty() {
                    extra_instructions.push(format!("## Agent Instructions\n\n{}", agents_md));
                }
            }

            // Inject MEMORY.md as persistent agent memory
            if let Some(memory_md) = loader.load_memory_md(&agent_workspace_dir, 20_000) {
                if !memory_md.is_empty() {
                    extra_instructions.push(format!("## Agent Memory\n\n{}", memory_md));
                }
            }

            // Inject recent daily memory logs
            let recent = loader.load_recent_memory(&agent_workspace_dir, 7);
            if !recent.is_empty() {
                let daily = recent.iter()
                    .map(|m| format!("### {}\n{}", m.date, m.content))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                extra_instructions.push(format!("## Recent Activity\n\n{}", daily));
            }

            // Load soul candidate from workspace (applied after soul variable is created)
            loader.load_soul(&agent_workspace_dir)
        };

        let custom_instructions = if extra_instructions.is_empty() {
            None
        } else {
            Some(extra_instructions.join("\n"))
        };

        // Only inject soul into prompts if bootstrap is complete (soul.md exists)
        let mut soul = if bootstrap_prompt.is_none() && !resolved_soul.is_empty() {
            Some(resolved_soul)
        } else {
            None
        };

        // Override soul manifest if workspace has SOUL.md
        if let Some(workspace_soul) = workspace_soul_candidate {
            soul = Some(workspace_soul);
        }

        let thinker_config = ThinkerConfig {
            prompt: PromptConfig {
                skill_instructions,
                runtime_capabilities,
                custom_instructions,
                ..PromptConfig::default()
            },
            soul,
            active_profile: Some(active_workspace.profile.clone()),
            ..ThinkerConfig::default()
        };
        let thinker = Arc::new(Thinker::new(thinker_registry, thinker_config));

        // Create Executor with routing support
        let local_executor = Arc::new(SingleStepExecutor::new(self.tool_registry.clone()));

        // Create compressor (no-op for Gateway mode)
        let compressor = Arc::new(NoOpCompressor);

        // Create AgentLoop with config
        let max_loops = agent.config().max_loops as usize;
        let loop_config = LoopConfig::default().with_max_steps(max_loops);

        // Build request context with workspace_id in metadata
        let mut context = RequestContext::empty();
        context.metadata.insert(
            "workspace_id".to_string(),
            active_workspace.workspace_id.clone(),
        );

        // Format history as initial summary
        let history_summary = self.format_history(&history);

        // Filter tools based on agent whitelist/blacklist
        let allowed_tools: Vec<UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();

        debug!(
            run_id = run_id,
            total_tools = self.tools.len(),
            allowed_tools = allowed_tools.len(),
            "Tool availability for agent loop"
        );

        // Run the loop (history_summary as Option<String>)
        let initial_history = if history_summary.is_empty() {
            None
        } else {
            Some(history_summary)
        };

        // Construct IdentityContext from session metadata
        let session_key_str = request.session_key.to_key_string();
        let identity = self
            .session_manager
            .get_identity_context(&session_key_str, "gateway")
            .await
            .unwrap_or_else(|_| {
                IdentityContext::owner(session_key_str.clone(), "gateway".to_string())
            });

        // Run with local executor
        let agent_loop = AgentLoop::new(thinker, local_executor, compressor, loop_config);
        // Inject task router for dynamic escalation
        let agent_loop = if let Some(ref router) = self.task_router {
            agent_loop.with_task_router(Arc::clone(router))
        } else {
            agent_loop
        };
        let mut run_context = RunContext::new(
            request.input.clone(),
            context,
            allowed_tools,
            identity.clone(),
        )
        .with_abort_signal(abort_rx);
        if let Some(history) = initial_history {
            run_context = run_context.with_initial_history(history);
        }
        let result = agent_loop
            .run(run_context, callback.as_ref())
            .await;

        // Update step count from result
        let steps = match &result {
            LoopResult::Completed { steps, .. } => *steps,
            LoopResult::Failed { steps, .. } => *steps,
            LoopResult::GuardTriggered(_) => 0,
            LoopResult::UserAborted => 0,
            LoopResult::PoeAborted { .. } => 0,
            LoopResult::Escalated { ref context, .. } => context.completed_steps,
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

                // Append session summary to daily memory log (only for real workspaces)
                if agent_workspace_dir.is_absolute() {
                    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
                    let time = chrono::Local::now().format("%H:%M").to_string();
                    let truncated_summary = if summary.len() > 500 {
                        format!("{}...", &summary[..summary.floor_char_boundary(500)])
                    } else {
                        summary.clone()
                    };
                    let entry = format!("\n## Session {}\n\n{}\n", time, truncated_summary);
                    let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());
                    if let Err(e) = loader.append_daily_memory(&agent_workspace_dir, &date, &entry) {
                        tracing::warn!(error = %e, "Failed to append daily memory");
                    }
                }

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
            LoopResult::PoeAborted { reason } => {
                warn!(run_id = %run_id, reason = %reason, "Agent loop aborted by POE");
                Err(ExecutionError::Failed(format!("POE aborted: {}", reason)))
            }
            LoopResult::Escalated { route, context } => {
                info!(
                    subsystem = "task_router",
                    event = "escalated",
                    run_id = %run_id,
                    route = route.label(),
                    completed_steps = context.completed_steps,
                    "Agent loop escalated to higher execution path"
                );
                // Phase 1: Return partial result with escalation info
                // Phase 2 will re-dispatch via route to Dispatcher/POE/Swarm
                let msg = context.partial_result.unwrap_or_else(|| {
                    format!(
                        "Task escalated to {} execution ({} steps completed). Full route dispatch coming in Phase 2.",
                        route.label(),
                        context.completed_steps
                    )
                });
                Ok(msg)
            }
        }
    }
}

// ============================================================================
// ExecutionAdapter trait implementation
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

// ============================================================================
// Background memory persistence
// ============================================================================

/// Write a conversation turn to the memory system (Layer 1).
///
/// Runs in a background task — failures are logged but never block the caller.
async fn write_conversation_memory(
    memory_backend: crate::memory::store::MemoryBackend,
    session_key: String,
    user_input: String,
    ai_output: String,
) {
    use crate::memory::context::{ContextAnchor, MemoryEntry};

    let context = ContextAnchor::with_topic(
        "aleph.chat".to_string(),
        session_key.clone(),
        session_key,
    );
    let entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        context,
        user_input,
        ai_output,
    );

    use crate::memory::store::SessionStore;
    if let Err(e) = memory_backend.insert_memory(&entry).await {
        warn!("Failed to write conversation memory: {}", e);
    } else {
        debug!("Conversation memory saved to Layer 1");
    }
}
