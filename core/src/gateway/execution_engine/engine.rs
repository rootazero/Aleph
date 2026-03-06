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
                ws.memory_filter = crate::gateway::workspace::WorkspaceFilter::Single(
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
        // Resolve AI identity from ~/.aleph/soul.md (layered: session > global > default)
        let identity_resolver = crate::thinker::identity::IdentityResolver::with_defaults();
        let resolved_soul = identity_resolver.resolve();
        let agent_id = request.session_key.agent_id().to_string();

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

        // ================================================================
        // Pre-classification: determine execution route before running
        // ================================================================
        let task_route = if let Some(ref router) = self.task_router {
            let router_context = crate::routing::RouterContext {
                session_history_len: 0,
                available_tools: allowed_tools.iter().map(|t| t.name.clone()).collect(),
                user_preferences: None,
            };
            let route = router.classify(&request.input, &router_context).await;
            info!(
                subsystem = "task_router",
                run_id = %run_id,
                route = route.label(),
                "Pre-classified task route"
            );
            route
        } else {
            crate::routing::TaskRoute::Simple
        };

        // ================================================================
        // Route dispatch: Simple runs AgentLoop; others dispatch to subsystems
        // ================================================================
        match task_route {
            crate::routing::TaskRoute::Simple => {
                // Standard Agent Loop path — clone params for potential escalation re-dispatch
                let result = self.execute_simple_loop(
                    run_id, request, &agent_workspace_dir, thinker.clone(), local_executor.clone(),
                    compressor.clone(), loop_config.clone(), context.clone(), allowed_tools.clone(),
                    identity.clone(), initial_history.clone(), abort_rx, callback.clone(),
                ).await;

                // Handle mid-execution escalation: re-dispatch to the escalated route
                match result {
                    Err(ExecutionError::Escalated { route, context: esc_ctx, .. }) => {
                        let enriched_input = if let Some(ref partial) = esc_ctx.partial_result {
                            format!(
                                "{}\n\n## Prior Analysis (from initial attempt)\n\n{}",
                                request.input, partial
                            )
                        } else {
                            request.input.clone()
                        };
                        let mut escalated_request = request.clone();
                        escalated_request.input = enriched_input;
                        // Use partial result as conversation context, or fall back to initial history
                        let escalated_history = esc_ctx.partial_result.clone().or(initial_history);

                        match route {
                            crate::routing::TaskRoute::MultiStep { ref reason } => {
                                info!(subsystem = "task_router", run_id = %run_id, reason = %reason,
                                    "Escalation re-dispatch → Dispatcher DAG");
                                match self.run_dispatcher_dag(
                                    run_id, &escalated_request, &agent_workspace_dir, thinker.clone(),
                                    local_executor.clone(), compressor.clone(), loop_config.clone(),
                                    context.clone(), allowed_tools.clone(), identity.clone(),
                                    escalated_history.clone(), callback.clone(),
                                ).await {
                                    Ok(r) => Ok(r),
                                    Err(e) => {
                                        warn!(subsystem = "task_router", run_id = %run_id, error = %e,
                                            "Escalated DAG failed, falling back to Agent Loop");
                                        self.execute_simple_loop(
                                            run_id, &escalated_request, &agent_workspace_dir, thinker,
                                            local_executor, compressor, loop_config, context,
                                            allowed_tools, identity, escalated_history,
                                            watch::channel(false).1, callback,
                                        ).await
                                    }
                                }
                            }
                            crate::routing::TaskRoute::Critical { ref reason, ref manifest_hints } => {
                                info!(subsystem = "task_router", run_id = %run_id, reason = %reason,
                                    "Escalation re-dispatch → POE Full");
                                match self.run_poe_critical(
                                    run_id, &escalated_request, &agent_workspace_dir, manifest_hints,
                                    thinker.clone(), local_executor.clone(), compressor.clone(),
                                    loop_config.clone(), allowed_tools.clone(), callback.clone(),
                                ).await {
                                    Ok(r) => Ok(r),
                                    Err(e) => {
                                        warn!(subsystem = "task_router", run_id = %run_id, error = %e,
                                            "Escalated POE failed, falling back to Agent Loop");
                                        self.execute_simple_loop(
                                            run_id, &escalated_request, &agent_workspace_dir, thinker,
                                            local_executor, compressor, loop_config, context,
                                            allowed_tools, identity, escalated_history,
                                            watch::channel(false).1, callback,
                                        ).await
                                    }
                                }
                            }
                            crate::routing::TaskRoute::Collaborative { ref reason, ref strategy } => {
                                info!(subsystem = "task_router", run_id = %run_id, reason = %reason,
                                    "Escalation re-dispatch → Collaborative");
                                match self.run_collaborative(
                                    run_id, &escalated_request, &agent_workspace_dir, strategy,
                                    thinker.clone(), local_executor.clone(), compressor.clone(),
                                    loop_config.clone(), context.clone(), allowed_tools.clone(),
                                    identity.clone(), callback.clone(),
                                ).await {
                                    Ok(r) => Ok(r),
                                    Err(e) => {
                                        warn!(subsystem = "task_router", run_id = %run_id, error = %e,
                                            "Escalated collaborative failed, falling back to Agent Loop");
                                        self.execute_simple_loop(
                                            run_id, &escalated_request, &agent_workspace_dir, thinker,
                                            local_executor, compressor, loop_config, context,
                                            allowed_tools, identity, escalated_history,
                                            watch::channel(false).1, callback,
                                        ).await
                                    }
                                }
                            }
                            crate::routing::TaskRoute::Simple => {
                                // Shouldn't escalate back to Simple, but handle gracefully
                                let partial = esc_ctx.partial_result.unwrap_or_default();
                                Ok(partial)
                            }
                        }
                    }
                    other => other,
                }
            }
            crate::routing::TaskRoute::MultiStep { ref reason } => {
                info!(
                    subsystem = "task_router",
                    run_id = %run_id,
                    reason = %reason,
                    "Dispatching to Dispatcher DAG"
                );
                // Dispatcher DAG: planner decomposes → DagScheduler executes
                match self.run_dispatcher_dag(
                    run_id, request, &agent_workspace_dir, thinker.clone(),
                    local_executor.clone(), compressor.clone(), loop_config.clone(),
                    context.clone(), allowed_tools.clone(), identity.clone(),
                    initial_history.clone(), callback.clone(),
                ).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        // Graceful degradation: DAG failure → fallback to Agent Loop
                        warn!(
                            subsystem = "task_router",
                            run_id = %run_id,
                            error = %e,
                            "DAG execution failed, falling back to Agent Loop"
                        );
                        self.execute_simple_loop(
                            run_id, request, &agent_workspace_dir, thinker, local_executor,
                            compressor, loop_config, context, allowed_tools, identity,
                            initial_history, abort_rx, callback,
                        ).await
                    }
                }
            }
            crate::routing::TaskRoute::Critical { ref reason, ref manifest_hints } => {
                info!(
                    subsystem = "task_router",
                    run_id = %run_id,
                    reason = %reason,
                    "Dispatching to POE Full Manager"
                );
                // POE Full: PoeManager wraps execution with SuccessManifest validation
                match self.run_poe_critical(
                    run_id, request, &agent_workspace_dir, manifest_hints,
                    thinker.clone(), local_executor.clone(), compressor.clone(),
                    loop_config.clone(), allowed_tools.clone(), callback.clone(),
                ).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        // Graceful degradation: POE failure → fallback to Agent Loop
                        warn!(
                            subsystem = "task_router",
                            run_id = %run_id,
                            error = %e,
                            "POE execution failed, falling back to Agent Loop"
                        );
                        self.execute_simple_loop(
                            run_id, request, &agent_workspace_dir, thinker, local_executor,
                            compressor, loop_config, context, allowed_tools, identity,
                            initial_history, abort_rx, callback,
                        ).await
                    }
                }
            }
            crate::routing::TaskRoute::Collaborative { ref reason, ref strategy } => {
                info!(
                    subsystem = "task_router",
                    run_id = %run_id,
                    reason = %reason,
                    strategy = ?strategy,
                    "Dispatching to collaborative execution"
                );
                // Collaborative: parallel agent execution with result aggregation
                match self.run_collaborative(
                    run_id, request, &agent_workspace_dir, strategy,
                    thinker.clone(), local_executor.clone(), compressor.clone(),
                    loop_config.clone(), context.clone(), allowed_tools.clone(),
                    identity.clone(), callback.clone(),
                ).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        // Graceful degradation: Swarm failure → fallback to DAG → Agent Loop
                        warn!(
                            subsystem = "task_router",
                            run_id = %run_id,
                            error = %e,
                            "Collaborative execution failed, falling back to Agent Loop"
                        );
                        self.execute_simple_loop(
                            run_id, request, &agent_workspace_dir, thinker, local_executor,
                            compressor, loop_config, context, allowed_tools, identity,
                            initial_history, abort_rx, callback,
                        ).await
                    }
                }
            }
        }
    }

    /// Execute the standard Agent Loop (Simple route).
    #[allow(clippy::too_many_arguments)]
    async fn execute_simple_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent_workspace_dir: &std::path::Path,
        thinker: Arc<Thinker<SingleProviderRegistry>>,
        executor: Arc<SingleStepExecutor<impl ToolRegistry + Send + Sync + 'static>>,
        compressor: Arc<NoOpCompressor>,
        loop_config: LoopConfig,
        context: RequestContext,
        allowed_tools: Vec<UnifiedTool>,
        identity: IdentityContext,
        initial_history: Option<String>,
        abort_rx: watch::Receiver<bool>,
        callback: Arc<EventEmittingCallback<E>>,
    ) -> Result<String, ExecutionError> {
        let agent_loop = AgentLoop::new(thinker, executor, compressor, loop_config);
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
                    if let Err(e) = loader.append_daily_memory(agent_workspace_dir, &date, &entry) {
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
            LoopResult::Escalated { route, context: esc_ctx } => {
                info!(
                    subsystem = "task_router",
                    event = "escalated",
                    run_id = %run_id,
                    route = route.label(),
                    completed_steps = esc_ctx.completed_steps,
                    "Agent loop escalated to higher execution path"
                );
                // Signal escalation back to caller for re-dispatch
                Err(ExecutionError::Escalated {
                    route_label: route.label().to_string(),
                    completed_steps: esc_ctx.completed_steps,
                    route,
                    context: esc_ctx,
                })
            }
        }
    }

    /// Execute via Dispatcher DAG: planner decomposes message into TaskGraph,
    /// DagScheduler executes nodes (each via Agent Loop).
    #[allow(clippy::too_many_arguments)]
    async fn run_dispatcher_dag<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent_workspace_dir: &std::path::Path,
        thinker: Arc<Thinker<SingleProviderRegistry>>,
        executor: Arc<SingleStepExecutor<impl ToolRegistry + Send + Sync + 'static>>,
        compressor: Arc<NoOpCompressor>,
        loop_config: LoopConfig,
        context: RequestContext,
        allowed_tools: Vec<UnifiedTool>,
        identity: IdentityContext,
        initial_history: Option<String>,
        _callback: Arc<EventEmittingCallback<E>>,
    ) -> Result<String, ExecutionError> {
        use crate::dispatcher::planner::{LlmTaskPlanner, TaskPlanner};
        use crate::dispatcher::scheduler::DagScheduler;
        use crate::dispatcher::context::TaskContext;
        use crate::dispatcher::callback::NoOpExecutionCallback;

        // Emit planning notification
        info!(subsystem = "task_router", run_id = %run_id, "Starting DAG planning");

        // 1. Plan: decompose message into TaskGraph
        let provider = self.provider_registry.default_provider();
        let planner = LlmTaskPlanner::new(provider);
        let graph = planner.plan(&request.input).await.map_err(|e| {
            ExecutionError::Failed(format!("Task planning failed: {}", e))
        })?;

        info!(
            subsystem = "task_router",
            run_id = %run_id,
            tasks = graph.tasks.len(),
            "Task graph planned"
        );

        info!(
            subsystem = "task_router",
            run_id = %run_id,
            task_count = graph.tasks.len(),
            "DAG scheduled, executing tasks"
        );

        // 2. Create a GraphTaskExecutor that uses Agent Loop for each node
        let dag_executor = Arc::new(AgentLoopGraphExecutor {
            thinker: thinker.clone(),
            executor: executor.clone(),
            compressor: compressor.clone(),
            loop_config: loop_config.clone(),
            tools: allowed_tools.clone(),
            identity: identity.clone(),
            workspace: agent_workspace_dir.to_path_buf(),
        });

        // 3. Execute via DagScheduler
        let task_context = TaskContext::new(&request.input);
        let dag_callback = Arc::new(NoOpExecutionCallback);
        let result = DagScheduler::execute_graph(
            graph, dag_executor, dag_callback, task_context, None,
        ).await.map_err(|e| {
            ExecutionError::Failed(format!("DAG execution failed: {}", e))
        })?;

        info!(
            subsystem = "task_router",
            run_id = %run_id,
            completed = result.completed_tasks.len(),
            failed = result.failed_tasks.len(),
            "DAG execution complete"
        );

        // 4. Aggregate results
        let summary = result.detailed_summary();
        Ok(summary)
    }

    /// Execute via POE Full Manager: wraps execution with SuccessManifest validation.
    #[allow(clippy::too_many_arguments)]
    async fn run_poe_critical<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent_workspace_dir: &std::path::Path,
        manifest_hints: &crate::routing::ManifestHints,
        thinker: Arc<Thinker<SingleProviderRegistry>>,
        executor: Arc<SingleStepExecutor<impl ToolRegistry + Send + Sync + 'static>>,
        compressor: Arc<NoOpCompressor>,
        loop_config: LoopConfig,
        allowed_tools: Vec<UnifiedTool>,
        _callback: Arc<EventEmittingCallback<E>>,
    ) -> Result<String, ExecutionError> {
        use crate::poe::{PoeManager, PoeConfig, CompositeValidator};
        use crate::poe::types::{PoeTask, PoeOutcome, SuccessManifest};
        use crate::poe::worker::AgentLoopWorker;

        info!(subsystem = "task_router", run_id = %run_id, "Building success manifest for POE");

        // 1. Build SuccessManifest from hints
        let mut manifest = SuccessManifest::new(run_id, &request.input);
        manifest.max_attempts = 3;

        // Add hard constraints from hints as semantic checks
        for constraint in &manifest_hints.hard_constraints {
            manifest.hard_constraints.push(
                crate::poe::types::ValidationRule::SemanticCheck {
                    target: crate::poe::types::JudgeTarget::Content(String::new()),
                    prompt: constraint.clone(),
                    passing_criteria: constraint.clone(),
                    model_tier: Default::default(),
                }
            );
        }

        // 2. Create PoeTask
        let task = PoeTask::new(manifest, request.input.clone());

        // 3. Create AgentLoopWorker as the POE worker
        let worker = AgentLoopWorker::new(
            agent_workspace_dir.to_path_buf(),
            thinker,
            executor,
            compressor,
            allowed_tools,
            loop_config,
        );

        // 4. Create PoeManager and execute
        let poe_provider = self.provider_registry.default_provider();
        let validator = CompositeValidator::new(poe_provider);
        let poe_config = PoeConfig::default();
        let manager = PoeManager::new(worker, validator, poe_config)
            .with_workspace(agent_workspace_dir.to_path_buf());

        info!(subsystem = "task_router", run_id = %run_id, "POE execution starting");

        let outcome = manager.execute(task).await.map_err(|e| {
            ExecutionError::Failed(format!("POE execution failed: {}", e))
        })?;

        info!(
            subsystem = "task_router",
            run_id = %run_id,
            "POE execution complete"
        );

        // 5. Convert outcome to response
        match outcome {
            PoeOutcome::Success { worker_summary, verdict } => {
                // Return worker's actual output; append verification status
                if worker_summary.is_empty() {
                    Ok(verdict.reason)
                } else {
                    Ok(worker_summary)
                }
            }
            PoeOutcome::StrategySwitch { reason, suggestion } => {
                Ok(format!(
                    "⚠️ POE 检测到执行策略需要调整: {}\n建议: {}",
                    reason, suggestion
                ))
            }
            PoeOutcome::BudgetExhausted { attempts, last_error } => {
                Ok(format!(
                    "⚠️ POE 已用完所有 {} 次重试，最后错误: {}",
                    attempts, last_error
                ))
            }
        }
    }

    /// Execute via collaborative multi-agent execution.
    #[allow(clippy::too_many_arguments)]
    async fn run_collaborative<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        _agent_workspace_dir: &std::path::Path,
        strategy: &crate::routing::CollabStrategy,
        thinker: Arc<Thinker<SingleProviderRegistry>>,
        executor: Arc<SingleStepExecutor<impl ToolRegistry + Send + Sync + 'static>>,
        compressor: Arc<NoOpCompressor>,
        loop_config: LoopConfig,
        context: RequestContext,
        allowed_tools: Vec<UnifiedTool>,
        identity: IdentityContext,
        callback: Arc<EventEmittingCallback<E>>,
    ) -> Result<String, ExecutionError> {
        info!(subsystem = "task_router", run_id = %run_id, "Starting collaborative execution");

        match strategy {
            crate::routing::CollabStrategy::Parallel => {
                // Parallel: run multiple Agent Loops with different role prompts
                self.run_parallel_agents(
                    run_id, request, thinker, executor, compressor,
                    loop_config, context, allowed_tools, identity, callback,
                ).await
            }
            crate::routing::CollabStrategy::Adversarial => {
                // Adversarial: Generator + Reviewer loop
                self.run_adversarial(
                    run_id, request, thinker, executor, compressor,
                    loop_config, context, allowed_tools, identity, callback,
                ).await
            }
            crate::routing::CollabStrategy::GroupChat => {
                // GroupChat: delegate to existing group_chat orchestrator
                // For now, fall back to simple execution (GroupChat has its own entry point)
                Err(ExecutionError::Failed(
                    "GroupChat is handled via dedicated chat channel".to_string()
                ))
            }
        }
    }

    /// Run parallel Agent Loops with different perspectives, then merge results.
    #[allow(clippy::too_many_arguments)]
    async fn run_parallel_agents<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        thinker: Arc<Thinker<SingleProviderRegistry>>,
        executor: Arc<SingleStepExecutor<impl ToolRegistry + Send + Sync + 'static>>,
        compressor: Arc<NoOpCompressor>,
        loop_config: LoopConfig,
        context: RequestContext,
        allowed_tools: Vec<UnifiedTool>,
        identity: IdentityContext,
        _callback: Arc<EventEmittingCallback<E>>,
    ) -> Result<String, ExecutionError> {
        let roles = vec![
            ("分析师", "你是一位深度分析师。从多个角度系统地分析以下任务，关注可行性、风险和最佳方案。"),
            ("执行者", "你是一位实干型执行者。直接给出具体的执行方案和操作步骤。"),
        ];

        info!(
            subsystem = "task_router",
            run_id = %run_id,
            agent_count = roles.len(),
            "Launching parallel agents"
        );

        let mut handles = Vec::new();

        for (role_name, role_prompt) in &roles {
            let thinker = thinker.clone();
            let executor = executor.clone();
            let compressor = compressor.clone();
            let config = loop_config.clone();
            let tools = allowed_tools.clone();
            let id = identity.clone();
            let input = format!(
                "## 角色: {}\n\n{}\n\n## 任务\n\n{}",
                role_name, role_prompt, request.input
            );
            let ctx = context.clone();
            let role = role_name.to_string();

            let handle = tokio::spawn(async move {
                let agent_loop = AgentLoop::new(thinker, executor, compressor, config);
                let run_ctx = RunContext::new(input, ctx, tools, id);
                let result = agent_loop.run(run_ctx, &crate::agent_loop::NoOpLoopCallback).await;
                (role, result)
            });
            handles.push(handle);
        }

        // Collect results
        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok((role, LoopResult::Completed { summary, .. })) => {
                    results.push(format!("### {} 视角\n\n{}", role, summary));
                }
                Ok((role, LoopResult::Failed { reason, .. })) => {
                    results.push(format!("### {} 视角\n\n⚠️ 执行失败: {}", role, reason));
                }
                Ok((role, _)) => {
                    results.push(format!("### {} 视角\n\n⚠️ 执行中断", role));
                }
                Err(e) => {
                    warn!(run_id = %run_id, error = %e, "Parallel agent task failed");
                }
            }
        }

        if results.is_empty() {
            return Err(ExecutionError::Failed("All parallel agents failed".to_string()));
        }

        info!(
            subsystem = "task_router",
            run_id = %run_id,
            agents = results.len(),
            "Parallel agent execution complete"
        );

        // Merge: use LLM to synthesize results
        let merged = format!(
            "# 多角色协作结果\n\n{}\n\n---\n\n*由 {} 个 Agent 协作完成*",
            results.join("\n\n---\n\n"),
            results.len()
        );
        Ok(merged)
    }

    /// Run adversarial execution: Generator produces result, Reviewer critiques.
    #[allow(clippy::too_many_arguments)]
    async fn run_adversarial<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        thinker: Arc<Thinker<SingleProviderRegistry>>,
        executor: Arc<SingleStepExecutor<impl ToolRegistry + Send + Sync + 'static>>,
        compressor: Arc<NoOpCompressor>,
        loop_config: LoopConfig,
        context: RequestContext,
        allowed_tools: Vec<UnifiedTool>,
        identity: IdentityContext,
        _callback: Arc<EventEmittingCallback<E>>,
    ) -> Result<String, ExecutionError> {
        let max_rounds = 2;
        let mut last_output = String::new();

        for round in 0..max_rounds {
            info!(
                subsystem = "task_router",
                run_id = %run_id,
                round = round + 1,
                max_rounds = max_rounds,
                "Adversarial round"
            );

            // Generator: execute the task (or revise based on feedback)
            let gen_input = if round == 0 {
                request.input.clone()
            } else {
                format!(
                    "## 原始任务\n\n{}\n\n## 上一轮输出\n\n{}\n\n## 审查反馈\n\n请根据以上反馈修正你的输出。",
                    request.input, last_output
                )
            };

            let gen_loop = AgentLoop::new(
                thinker.clone(), executor.clone(), compressor.clone(), loop_config.clone(),
            );
            let gen_ctx = RunContext::new(
                gen_input, context.clone(), allowed_tools.clone(), identity.clone(),
            );
            let gen_result = gen_loop.run(gen_ctx, &crate::agent_loop::NoOpLoopCallback).await;

            let gen_output = match gen_result {
                LoopResult::Completed { summary, .. } => summary,
                LoopResult::Failed { reason, .. } => {
                    return Err(ExecutionError::Failed(format!("Generator failed: {}", reason)));
                }
                _ => return Err(ExecutionError::Failed("Generator interrupted".to_string())),
            };

            // Reviewer: critique the output
            let review_input = format!(
                "## 角色: 严格审查员\n\n审查以下输出是否完整、准确、高质量。如果有问题，指出具体问题和改进建议。如果质量达标，回复 'APPROVED'。\n\n## 原始任务\n\n{}\n\n## 待审查输出\n\n{}",
                request.input, gen_output
            );

            let rev_loop = AgentLoop::new(
                thinker.clone(), executor.clone(), compressor.clone(), loop_config.clone(),
            );
            let rev_ctx = RunContext::new(
                review_input, context.clone(), allowed_tools.clone(), identity.clone(),
            );
            let rev_result = rev_loop.run(rev_ctx, &crate::agent_loop::NoOpLoopCallback).await;

            let review = match rev_result {
                LoopResult::Completed { summary, .. } => summary,
                _ => "APPROVED".to_string(), // If reviewer fails, accept
            };

            if review.contains("APPROVED") {
                info!(
                    subsystem = "task_router",
                    run_id = %run_id,
                    round = round + 1,
                    "Adversarial review approved"
                );
                return Ok(gen_output);
            }

            last_output = format!("{}\n\n**审查反馈:** {}", gen_output, review);
        }

        // Return last output after max rounds
        Ok(last_output)
    }
}

// ============================================================================
// AgentLoopGraphExecutor — enables DAG nodes to be executed via Agent Loop
// ============================================================================

/// GraphTaskExecutor implementation that runs each DAG node via an Agent Loop.
struct AgentLoopGraphExecutor<T, E, C>
where
    T: crate::agent_loop::ThinkerTrait + 'static,
    E: crate::agent_loop::ActionExecutor + 'static,
    C: crate::agent_loop::CompressorTrait + 'static,
{
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    loop_config: LoopConfig,
    tools: Vec<UnifiedTool>,
    identity: IdentityContext,
    workspace: std::path::PathBuf,
}

#[async_trait]
impl<T, E, C> crate::dispatcher::scheduler::GraphTaskExecutor for AgentLoopGraphExecutor<T, E, C>
where
    T: crate::agent_loop::ThinkerTrait + 'static,
    E: crate::agent_loop::ActionExecutor + 'static,
    C: crate::agent_loop::CompressorTrait + 'static,
{
    async fn execute(
        &self,
        task: &crate::dispatcher::agent_types::Task,
        context: &str,
    ) -> crate::error::Result<crate::dispatcher::context::TaskOutput> {
        let prompt = if context.is_empty() {
            task.name.clone()
        } else {
            format!("{}\n\n## Context from dependencies\n\n{}", task.name, context)
        };

        let mut loop_config = self.loop_config.clone();
        // DAG nodes should not re-escalate — disable task router
        loop_config.max_steps = 30;

        let agent_loop = AgentLoop::new(
            self.thinker.clone(),
            self.executor.clone(),
            self.compressor.clone(),
            loop_config,
        );
        let context = RequestContext {
            working_directory: Some(self.workspace.to_string_lossy().to_string()),
            ..Default::default()
        };
        let run_ctx = RunContext::new(
            prompt,
            context,
            self.tools.clone(),
            self.identity.clone(),
        );
        let result = agent_loop
            .run(run_ctx, &crate::agent_loop::NoOpLoopCallback)
            .await;

        match result {
            LoopResult::Completed { summary, .. } => {
                info!(subsystem = "dag_executor", task_id = %task.id, "DAG node completed");
                Ok(crate::dispatcher::context::TaskOutput::text(summary))
            }
            LoopResult::Failed { reason, .. } => {
                warn!(subsystem = "dag_executor", task_id = %task.id, reason = %reason, "DAG node failed");
                Err(crate::error::AlephError::other(format!(
                    "Agent loop failed for task '{}': {}",
                    task.id, reason
                )))
            }
            LoopResult::Escalated { route, .. } => {
                // DAG nodes should complete, not escalate; treat as partial success
                info!(subsystem = "dag_executor", task_id = %task.id, route = route.label(), "DAG node tried to escalate, treating as completion");
                Ok(crate::dispatcher::context::TaskOutput::text(
                    format!("Task '{}' partially completed (escalation suppressed)", task.id)
                ))
            }
            LoopResult::GuardTriggered(violation) => {
                warn!(subsystem = "dag_executor", task_id = %task.id, violation = %violation.description(), "DAG node guard triggered");
                Err(crate::error::AlephError::other(format!(
                    "Guard triggered for task '{}': {}",
                    task.id, violation.description()
                )))
            }
            LoopResult::UserAborted => {
                warn!(subsystem = "dag_executor", task_id = %task.id, "DAG node user aborted");
                Err(crate::error::AlephError::other(format!(
                    "User aborted task '{}'", task.id
                )))
            }
            LoopResult::PoeAborted { reason } => {
                warn!(subsystem = "dag_executor", task_id = %task.id, reason = %reason, "DAG node POE aborted");
                Err(crate::error::AlephError::other(format!(
                    "POE aborted task '{}': {}", task.id, reason
                )))
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
