//! ExecutionEngine — bridges Gateway requests to the AgentLoop.

use std::collections::HashMap;
use crate::sync_primitives::{AtomicU32, AtomicU64};
use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::{ActiveRun, ExecutionEngineConfig, ExecutionError, RunRequest, RunState, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, AgentState, MessageRole};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, RunSummary, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::gateway::workspace::WorkspaceManager;

use crate::dispatcher::UnifiedTool;
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

/// Execution engine that bridges Gateway to the AgentLoop
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
    /// Compression service for turn-based fact extraction
    compression_service: Option<Arc<crate::memory::compression::CompressionService>>,
    /// Memory context provider for LanceDB-backed prompt augmentation
    memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Create a new execution engine
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
            compression_service: None,
            memory_context_provider: None,
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

    /// Set the workspace manager for workspace-scoped profile resolution.
    ///
    /// When set, the engine resolves the user's active workspace at the start
    /// of each run and injects the workspace profile into the prompt builder
    /// and the workspace_id into the request context metadata.
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
        // tools (agent_create, agent_switch) can auto-switch correctly.
        // ================================================================
        if let Some(sc_handle) = self.tool_registry.session_context_handle() {
            let mut sc = sc_handle.write().await;
            sc.channel = request.metadata.get("channel_id").cloned().unwrap_or_default();
            sc.peer_id = request.metadata.get("sender_id").cloned().unwrap_or_default();
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
                    // Mark run as completed and finalize
                    let (started_at, steps_completed, final_seq) = {
                        let mut runs = self.active_runs.write().await;
                        if let Some(run) = runs.get_mut(&run_id) {
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
                            run_id: run_id.clone(),
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
                    return Ok(());
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
                        // to prevent agent loop from processing slash commands as plain text.
                        let (started_at, final_seq) = {
                            let mut runs = self.active_runs.write().await;
                            if let Some(run) = runs.get_mut(&run_id) {
                                run.state = RunState::Completed;
                                run.cancel_tx = None;
                                (run.started_at, run.next_seq())
                            } else {
                                (chrono::Utc::now(), 0)
                            }
                        };

                        agent.set_state(AgentState::Idle).await;
                        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
                        let error_response = format!("❌ {}", error_msg);

                        agent
                            .add_message(&request.session_key, MessageRole::Assistant, &error_response)
                            .await;
                        let _ = emitter
                            .emit(StreamEvent::ResponseChunk {
                                run_id: run_id.clone(),
                                seq: 1,
                                content: error_response.clone(),
                                chunk_index: 0,
                                is_final: true,
                            })
                            .await;
                        let _ = emitter
                            .emit(StreamEvent::RunComplete {
                                run_id: run_id.clone(),
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
                        return Ok(());
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

    /// List active runs

    /// Try to resolve a `/command args` input to a slash command mode JSON.
    ///
    /// Used for non-router paths (Panel, CLI) where the inbound router's
    /// command resolution doesn't run. Returns `Some(mode_json)` if the
    /// command matches a registered tool, `None` otherwise.
    fn try_resolve_slash_command(&self, input: &str) -> Option<String> {
        let trimmed = input.trim();
        let without_slash = trimmed.strip_prefix('/')?;
        if without_slash.is_empty() {
            return None;
        }

        let (cmd_name, args) = match without_slash.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.to_lowercase(), rest.trim().to_string()),
            None => (without_slash.to_lowercase(), String::new()),
        };

        // Strip @botname suffix (e.g. "gen@mybot" → "gen")
        let cmd_name = match cmd_name.split_once('@') {
            Some((name, _)) => name.to_string(),
            None => cmd_name,
        };

        // Map common shorthand commands to their actual tool names
        let cmd_name = match cmd_name.as_str() {
            "switch" => "agent_switch".to_string(),
            other => other.to_string(),
        };

        // Check if this matches a registered tool
        if self.tool_registry.get_tool(&cmd_name).is_some() {
            let mode = serde_json::json!({
                "type": "direct_tool",
                "tool_id": cmd_name,
                "args": args,
            });
            let mode_json = serde_json::to_string(&mode).ok()?;
            info!(
                "[Engine] Inline slash command resolved: /{}",
                cmd_name
            );
            Some(mode_json)
        } else {
            None
        }
    }

    /// Execute a slash command directly, bypassing the full agent loop.
    ///
    /// This is the L0 fast path: parse the serialized execution mode from metadata,
    /// call the tool via the tool registry, and stream the result back.
    /// Falls back to an error if the tool is not found or execution fails.
    async fn execute_slash_command_fast_path<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        mode_json: &str,
        request: &RunRequest,
        _agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        let mode: serde_json::Value = serde_json::from_str(mode_json)
            .map_err(|e| ExecutionError::Failed(format!("Invalid slash command metadata: {}", e)))?;

        let mode_type = mode["type"].as_str().unwrap_or("");

        info!(
            run_id = %run_id,
            mode_type = %mode_type,
            "Slash command fast path executing"
        );

        match mode_type {
            "direct_tool" => {
                let tool_id = mode["tool_id"]
                    .as_str()
                    .ok_or_else(|| ExecutionError::Failed("Missing tool_id".to_string()))?;
                let args_str = mode["args"].as_str().unwrap_or("");

                // Build tool arguments — map slash command args to the
                // correct field names for each tool's expected schema.
                let arguments = match tool_id {
                    "agent_switch" | "agent_delete" => serde_json::json!({
                        "agent_id": args_str,
                    }),
                    "agent_create" => serde_json::json!({
                        "input": args_str,
                    }),
                    // URL-based tools
                    "browser_open" | "web_fetch" => serde_json::json!({
                        "url": args_str,
                    }),
                    // Selector-based browser tools
                    "browser_click" | "browser_select" => serde_json::json!({
                        "selector": args_str,
                    }),
                    "browser_type" => {
                        // /browser_type <selector> <text>
                        let (sel, txt) = args_str.split_once(' ')
                            .unwrap_or((args_str, ""));
                        serde_json::json!({
                            "selector": sel,
                            "text": txt,
                        })
                    }
                    "browser_evaluate" => serde_json::json!({
                        "script": args_str,
                    }),
                    // Query-based tools
                    "search" | "memory_search" => serde_json::json!({
                        "query": args_str,
                    }),
                    // Tabs: action is required, default to "list"
                    "browser_tabs" => serde_json::json!({
                        "action": if args_str.is_empty() { "list" } else { args_str },
                    }),
                    // Navigate: action is required, default to "refresh"
                    "browser_navigate" => serde_json::json!({
                        "action": if args_str.is_empty() { "refresh" } else { args_str },
                    }),
                    // Tools with no required args
                    "browser_screenshot" | "browser_snapshot"
                    | "browser_profile" => {
                        if args_str.is_empty() {
                            serde_json::json!({})
                        } else {
                            serde_json::json!({ "input": args_str })
                        }
                    }
                    _ => serde_json::json!({
                        "input": args_str,
                        "query": args_str,
                        "args": args_str,
                        "input_text": request.input,
                    }),
                };

                // Emit reasoning event
                let _ = emitter
                    .emit(StreamEvent::Reasoning {
                        run_id: run_id.to_string(),
                        seq: 0,
                        content: format!("Executing /{} ...", tool_id),
                        is_complete: true,
                    })
                    .await;

                // Execute the tool directly
                match self.tool_registry.execute_tool(tool_id, arguments).await {
                    Ok(result) => {
                        // Prefer _display field for human-readable output,
                        // then message field, then string value, then JSON
                        let response = if let Some(display) = result.get("_display").and_then(|v| v.as_str()) {
                            display.to_string()
                        } else if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
                            msg.to_string()
                        } else if let Some(s) = result.as_str() {
                            s.to_string()
                        } else {
                            serde_json::to_string_pretty(&result).unwrap_or_default()
                        };

                        // Stream response
                        let _ = emitter
                            .emit(StreamEvent::ResponseChunk {
                                run_id: run_id.to_string(),
                                seq: 1,
                                content: response.clone(),
                                chunk_index: 0,
                                is_final: true,
                            })
                            .await;

                        Ok(response)
                    }
                    Err(e) => {
                        Err(ExecutionError::Failed(format!(
                            "Tool '{}' execution failed: {}",
                            tool_id, e
                        )))
                    }
                }
            }

            "skill" => {
                // For skills, construct a focused prompt and route through a single-step agent loop
                let skill_name = mode["display_name"].as_str().unwrap_or("skill");
                let instructions = mode["instructions"].as_str().unwrap_or("");
                let args = mode["args"].as_str().unwrap_or("");

                // For skills we fall through to the agent loop with modified input
                // since skills need LLM processing with injected instructions
                Err(ExecutionError::Failed(format!(
                    "SKILL_FALLTHROUGH:{}:{}:{}",
                    skill_name, instructions, args
                )))
            }

            "mcp" => {
                let server_name = mode["server_name"].as_str().unwrap_or("");
                let tool_name = mode["tool_name"].as_str();
                let mcp_tool_id = if let Some(tn) = tool_name {
                    format!("mcp__{}_{}", server_name, tn)
                } else {
                    format!("mcp__{}", server_name)
                };
                let args_str = mode["args"].as_str().unwrap_or("");

                let arguments = serde_json::json!({
                    "input": args_str,
                    "args": args_str,
                    "input_text": request.input,
                });

                let _ = emitter
                    .emit(StreamEvent::Reasoning {
                        run_id: run_id.to_string(),
                        seq: 0,
                        content: format!("Executing MCP tool /{} ...", server_name),
                        is_complete: true,
                    })
                    .await;

                match self.tool_registry.execute_tool(&mcp_tool_id, arguments).await {
                    Ok(result) => {
                        let response = if let Some(s) = result.as_str() {
                            s.to_string()
                        } else {
                            serde_json::to_string_pretty(&result).unwrap_or_default()
                        };

                        let _ = emitter
                            .emit(StreamEvent::ResponseChunk {
                                run_id: run_id.to_string(),
                                seq: 1,
                                content: response.clone(),
                                chunk_index: 0,
                                is_final: true,
                            })
                            .await;

                        Ok(response)
                    }
                    Err(e) => Err(ExecutionError::Failed(format!(
                        "MCP tool '{}' execution failed: {}",
                        mcp_tool_id, e
                    ))),
                }
            }

            "custom" => {
                // Custom commands need LLM with a custom system prompt — fall through
                Err(ExecutionError::Failed("CUSTOM_FALLTHROUGH".to_string()))
            }

            _ => {
                Err(ExecutionError::Failed(format!(
                    "Unknown slash command type: {}",
                    mode_type
                )))
            }
        }
    }
    /// Run the agent loop (think→act two-step, Claude Code-inspired).
    ///
    /// Uses the flat `LoopToolRegistry` and single-layer `SafetyGuard`.
    async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
    ) -> Result<String, ExecutionError> {
        use crate::agent_loop::{
            AgentLoop, PromptBuilder, SafetyGuard, LoopConfig,
            adapters::build_registry_from_tools,
            provider_bridge::AiProviderBridge,
        };

        info!(run_id = run_id, "Starting agent loop (think→act)");

        // Get provider
        let provider = self.provider_registry.default_provider();
        let bridge = AiProviderBridge::new(provider);

        // Build tool registry from UnifiedTool list (filtered by agent whitelist)
        let allowed_tools: Vec<UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();

        let tool_registry = build_registry_from_tools(
            self.tool_registry.clone(),
            &allowed_tools,
        );

        debug!(
            run_id = run_id,
            tool_count = tool_registry.len(),
            "Agent loop: built tool registry"
        );

        // Resolve soul for prompt building
        let identity_resolver = crate::thinker::identity::IdentityResolver::with_defaults();
        let resolved_soul = identity_resolver.resolve();
        let prompt_builder = if resolved_soul.is_empty() {
            PromptBuilder::new()
        } else {
            PromptBuilder::from_soul(&resolved_soul)
        };

        // Safety guard with defaults
        let safety = SafetyGuard::default_guard();

        // Config from agent
        let max_loops = agent.config().max_loops as usize;
        let timeout_secs = request
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);
        let loop_config = LoopConfig {
            max_iterations: max_loops,
            token_budget: agent.config().max_tokens.unwrap_or(200_000) as usize,
            timeout_secs,
        };

        // Create and run the agent loop
        let agent_loop = AgentLoop::new(
            bridge,
            tool_registry,
            prompt_builder,
            safety,
            loop_config,
        );

        // Load conversation history from session (for multi-turn context)
        let history = {
            use crate::agent_loop::LoopMessage;
            use crate::gateway::agent_instance::MessageRole;

            let session_history = agent.get_history(&request.session_key, Some(50)).await;
            // Convert SessionMessage to LoopMessage, excluding the current user input
            // (which was just added above and will be appended by run_with_history)
            let mut msgs: Vec<LoopMessage> = Vec::new();
            // Skip the last message if it's the current user input we just stored
            let history_slice = if session_history.last().map(|m| {
                m.role == MessageRole::User && m.content == request.input
            }).unwrap_or(false) {
                &session_history[..session_history.len() - 1]
            } else {
                &session_history
            };
            for msg in history_slice {
                match msg.role {
                    MessageRole::User => msgs.push(LoopMessage::User(msg.content.clone())),
                    MessageRole::Assistant => msgs.push(LoopMessage::Assistant(msg.content.clone())),
                    _ => {}
                }
            }
            msgs
        };

        // Create a streaming callback that emits events
        let mut callback = StreamCallback::new(emitter.clone(), run_id.to_string());

        match agent_loop.run_with_history(&request.input, history, &mut callback).await {
            Ok(result) => {
                info!(
                    run_id = run_id,
                    iterations = result.iterations,
                    tool_calls = result.tool_calls_made,
                    tokens = result.total_tokens,
                    "Agent loop completed"
                );
                Ok(result.final_text.unwrap_or_default())
            }
            Err(e) => {
                error!(run_id = run_id, error = %e, "Agent loop failed");
                Err(ExecutionError::Failed(e.to_string()))
            }
        }
    }
}

/// Callback adapter that bridges AgentLoop events to Gateway StreamEvents.
struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    seq: u64,
    chunk_index: u32,
}

impl<E: EventEmitter + Send + Sync + 'static> StreamCallback<E> {
    fn new(emitter: Arc<E>, run_id: String) -> Self {
        Self {
            emitter,
            run_id,
            seq: 0,
            chunk_index: 0,
        }
    }
}

impl<E: EventEmitter + Send + Sync + 'static> crate::agent_loop::LoopCallback
    for StreamCallback<E>
{
    fn on_text(&mut self, text: &str) {
        self.seq += 1;
        let chunk_index = self.chunk_index;
        self.chunk_index += 1;

        let event = StreamEvent::ResponseChunk {
            run_id: self.run_id.clone(),
            seq: self.seq,
            content: text.to_string(),
            chunk_index,
            is_final: false,
        };

        // Fire-and-forget emit (LoopCallback is sync, emitter is async)
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            let _ = emitter.emit(event).await;
        });
    }

    fn on_tool_start(&mut self, name: &str, input: &serde_json::Value) {
        self.seq += 1;
        let event = StreamEvent::ToolStart {
            run_id: self.run_id.clone(),
            seq: self.seq,
            tool_name: name.to_string(),
            tool_id: name.to_string(),
            params: input.clone(),
        };
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            let _ = emitter.emit(event).await;
        });
    }

    fn on_tool_done(&mut self, name: &str, result: &crate::agent_loop::ToolResult) {
        use crate::gateway::event_emitter::ToolResult as EmitterToolResult;
        self.seq += 1;
        let tool_result = match result {
            crate::agent_loop::ToolResult::Success { output } => {
                EmitterToolResult::success(output.to_string())
            }
            crate::agent_loop::ToolResult::Error { error, .. } => {
                EmitterToolResult::error(error.clone())
            }
        };
        let event = StreamEvent::ToolEnd {
            run_id: self.run_id.clone(),
            seq: self.seq,
            tool_id: name.to_string(),
            result: tool_result,
            duration_ms: 0,
        };
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            let _ = emitter.emit(event).await;
        });
    }
}


// ============================================================================
// ExecutionAdapter trait implementation
// ============================================================================

/// Implement ExecutionAdapter for the ExecutionEngine.
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
    agent_id: String,
    user_input: String,
    ai_output: String,
) {
    use crate::memory::context::{ContextAnchor, MemoryEntry};

    let context = ContextAnchor::with_topic(
        session_key.clone(),
        session_key,
    );
    let mut entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        context,
        user_input,
        ai_output,
    );
    entry.workspace = agent_id;

    use crate::memory::store::SessionStore;
    if let Err(e) = memory_backend.insert_memory(&entry).await {
        warn!("Failed to write conversation memory: {}", e);
    } else {
        debug!("Conversation memory saved to Layer 1");
    }
}
