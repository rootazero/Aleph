//! Agent Handlers
//!
//! RPC handlers for agent operations: run, wait, cancel, status.

use crate::sync_primitives::Arc;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::agent_instance::AgentRegistry;
use super::super::event_bus::GatewayEventBus;
use super::super::event_emitter::{EventEmitter, GatewayEventEmitter, OutputMode, StreamEvent};
use super::super::execution_adapter::ExecutionAdapter;
use super::super::execution_engine::RunRequest;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::router::{AgentRouter, SessionKey};
use super::parse_params;

/// A file attachment sent with a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// File name
    pub name: String,
    /// MIME type (e.g., "image/png", "application/pdf")
    pub mime_type: String,
    /// Base64-encoded file content
    pub data: String,
}

/// Parameters for agent.run request
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRunParams {
    /// User input message
    pub input: String,
    /// Optional session key (auto-generated if not provided)
    #[serde(default)]
    pub session_key: Option<String>,
    /// Channel identifier (e.g., "gui:window1", "cli:term1")
    #[serde(default)]
    pub channel: Option<String>,
    /// Peer identifier for per-peer sessions
    #[serde(default)]
    pub peer_id: Option<String>,
    /// Whether to stream events (default: true)
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// Thinking level for LLM reasoning depth
    ///
    /// Supports: "off", "minimal", "low", "medium", "high", "xhigh"
    /// Also supports aliases: "think", "ultrathink", "max", etc.
    /// Default is "minimal" if not specified.
    #[serde(default)]
    pub thinking: Option<String>,
    /// File attachments sent with the message
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Explicit target agent ID (bypasses channel binding resolution)
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Optional absolute project root. When set, the agent's tool calls
    /// run inside this directory instead of `~/.aleph/workspaces/{agent_id}`.
    /// See `gateway/handlers/chat.rs` for the user-facing flow.
    #[serde(default)]
    pub project_root: Option<String>,
    /// Per-turn model override forwarded from `chat.send`. Lights up the
    /// chat-window model picker — see
    /// [`crate::gateway::model_override::ModelOverride`].
    #[serde(default)]
    pub model_override: Option<crate::gateway::model_override::ModelOverride>,
    /// Marks this run's user input as ASR-transcribed speech (the Panel voice
    /// loop). Wires the session voice-mode registry so `VoiceModeLayer`
    /// injects spoken-reply guidance into this turn's prompt, and applies the
    /// `[voice]` low-TTFT model pin when no explicit override is set —
    /// mirroring the channel voice path. Recomputed every turn: a typed turn
    /// (default `false`) clears the flag.
    #[serde(default)]
    pub voice_input: bool,
}

const fn default_stream() -> bool {
    true
}

/// Result of agent.run request (immediate response)
#[derive(Debug, Clone, Serialize)]
pub struct AgentRunResult {
    /// Unique run identifier
    pub run_id: String,
    /// Resolved session key
    pub session_key: String,
    /// Timestamp when accepted
    pub accepted_at: String,
}

/// Run state for tracking active runs
#[derive(Debug, Clone)]
pub struct RunState {
    pub run_id: String,
    pub session_key: SessionKey,
    pub started_at: Instant,
    pub status: RunStatus,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

/// Manager for agent runs
pub struct AgentRunManager {
    router: Arc<AgentRouter>,
    event_bus: Arc<GatewayEventBus>,
    active_runs: Arc<RwLock<HashMap<String, RunState>>>,
    agent_registry: Arc<AgentRegistry>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    /// Live application config, used to read `behavior.output_mode` fresh per
    /// run so the global typewriter/instant switch reaches Panel runs too.
    /// `None` for test/host-only constructions → falls back to typewriter,
    /// matching the inbound channel path (`inbound_router::executor`).
    app_config: Option<Arc<RwLock<crate::Config>>>,
}

impl AgentRunManager {
    pub fn new(
        router: Arc<AgentRouter>,
        event_bus: Arc<GatewayEventBus>,
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
    ) -> Self {
        Self {
            router,
            event_bus,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            agent_registry,
            execution_adapter,
            app_config: None,
        }
    }

    /// Inject the live application config so Panel runs honor the global
    /// `behavior.output_mode` toggle. Mirrors
    /// `InboundRouterExecutor::with_app_config`.
    pub fn with_app_config(mut self, config: Arc<RwLock<crate::Config>>) -> Self {
        self.app_config = Some(config);
        self
    }

    /// Resolve the global output mode fresh from the live config. Reads the
    /// same `behavior.output_mode` key the chat channels read, so a runtime
    /// toggle takes effect on the very next Panel run with no restart. Absent
    /// config (tests, host-only) defaults to typewriter.
    async fn resolved_output_mode(&self) -> OutputMode {
        match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                OutputMode::from_config(
                    cfg.behavior
                        .as_ref()
                        .map_or("typewriter", |b| b.output_mode.as_str()),
                )
            }
            None => OutputMode::Typewriter,
        }
    }

    /// Start a new agent run
    pub async fn start_run(&self, params: AgentRunParams) -> Result<AgentRunResult, String> {
        // Generate run ID
        let run_id = uuid::Uuid::new_v4().to_string();

        // Resolve session key
        let session_key = self
            .router
            .route(
                params.session_key.as_deref(),
                params.channel.as_deref(),
                params.peer_id.as_deref(),
                params.agent_id.as_deref(),
            )
            .await;

        let session_key_str = session_key.to_key_string();
        let accepted_at = chrono::Utc::now().to_rfc3339();

        // Record voice mode for this session BEFORE the run spawns, so prompt
        // assembly (`VoiceModeLayer` via the harness bridge) sees it on THIS
        // turn. Same registry the channel inbound path writes
        // (`inbound_router::executor`); channel sessions use distinct keys, so
        // the unconditional per-turn recompute here cannot clobber them.
        crate::gateway::voice::session_mode::set(
            &session_key_str,
            params.voice_input,
            params.voice_input,
        );

        // Create run state
        let run_state = RunState {
            run_id: run_id.clone(),
            session_key: session_key.clone(),
            started_at: Instant::now(),
            status: RunStatus::Running,
            input: params.input.clone(),
        };

        // Store in active runs
        {
            let mut runs = self.active_runs.write().await;
            runs.insert(run_id.clone(), run_state);
        }

        info!("Started run {} for session {}", run_id, session_key_str);

        // Emit run accepted event
        if params.stream {
            let event = StreamEvent::RunAccepted {
                run_id: run_id.clone(),
                session_key: session_key_str.clone(),
                accepted_at: accepted_at.clone(),
            };

            if let Ok(event_value) = serde_json::to_value(&event) {
                let notification = super::super::protocol::JsonRpcRequest::notification(
                    "stream.run_accepted",
                    Some(event_value),
                );
                if let Ok(json) = serde_json::to_string(&notification) {
                    self.event_bus.publish(json);
                }
            }
        }

        let agent_id = session_key.agent_id();
        let agent = match self.agent_registry.get(agent_id).await {
            Some(a) => a,
            None => {
                error!("Agent not found: {}", agent_id);
                let mut runs = self.active_runs.write().await;
                if let Some(run) = runs.get_mut(&run_id) {
                    run.status = RunStatus::Failed(format!("Agent not found: {agent_id}"));
                }
                return Err(format!("Agent not found: {agent_id}"));
            }
        };

        let mut metadata = HashMap::new();
        metadata.insert("channel_id".to_string(), params.channel.unwrap_or_default());
        metadata.insert("sender_id".to_string(), "websocket".to_string());
        // The Panel is a WebRich surface. Stamp `platform=webchat` so the
        // run-loop (`run_loop::inner`) derives a WebRich InteractionManifest.
        // Without it `metadata["platform"]` is absent → the manifest is `None`
        // → `prompt_build` falls back to the `Background` paradigm, whose
        // `SilentReply` capability makes `ProtocolTokensLayer` teach the model
        // `ALEPH_SILENT_COMPLETE`. The model then emits that literal token as a
        // silent completion and it leaks verbatim into the chat bubble — the
        // interactive path has no protocol-token suppression (only cron's
        // `deliverable_text` strips it). Mirrors the team-broadcast
        // (`member_run_metadata`) and channel-inbound paths that already tag
        // platform; this direct Panel-run path was the one that missed it.
        metadata.insert("platform".to_string(), "webchat".to_string());
        if let Some(peer_id) = &params.peer_id {
            metadata.insert("peer_id".to_string(), peer_id.clone());
        }

        // Stamp the originating connection's authorization role (set by the
        // gateway dispatch loop via CALLER_ROLE) so the tool-dispatch tier gate
        // can reject config-mutating tools for chat-tier devices. Covers BOTH
        // chat.send and agent.run since both reach here via start_run in the
        // same task. Absent for non-gateway runs (cron/internal) and for the
        // local no-auth daemon → the gate treats those as trusted.
        if let Some(role) = crate::gateway::caller_identity::current_caller_role() {
            metadata.insert("caller_role".to_string(), role);
        }

        // Validate and resolve optional project_root. We refuse anything that
        // isn't an existing absolute directory so the rest of the engine can
        // assume the override is safe to chdir/scan into.
        let workspace_override = match params.project_root.as_deref() {
            Some(raw) => {
                // Layer-2 (config tier) gate: choosing/creating a working
                // directory is a config-tier capability. A chat-tier caller
                // (remote Panel paired at "chat", or an external channel stamped
                // "guest") is locked to its default workspace and may not pick an
                // arbitrary project_root. Absent role (trusted local/internal
                // run) and "operator" pass — mirrors
                // `TurnContext::caller_is_operator`.
                //
                // Desktop App: the local Panel runs over loopback. Allow a
                // project_root override from any loopback connection regardless
                // of pairing tier — on the desktop the local operator IS the
                // user. Remote LAN connections (non-loopback, chat-tier) stay
                // gated, so opening `[gateway] host = "0.0.0.0"` does not hand
                // arbitrary working-directory selection to every LAN device.
                let role = crate::gateway::caller_identity::current_caller_role();
                let is_config_tier = !matches!(role.as_deref(), Some(r) if r != "operator");
                let is_loopback = crate::gateway::caller_identity::current_caller_is_loopback();
                if !is_config_tier && !is_loopback {
                    return Err(format!(
                        "choosing a working directory requires config-tier authorization \
                         or a local (loopback) connection; this connection is paired at \
                         chat level from a remote address (project_root: {raw})"
                    ));
                }
                let path = std::path::PathBuf::from(raw);
                if !path.is_absolute() {
                    return Err(format!("project_root must be absolute: {raw}"));
                }
                if !path.is_dir() {
                    return Err(format!("project_root is not a directory: {raw}"));
                }
                metadata.insert("project_root".to_string(), path.display().to_string());
                Some(path)
            }
            None => None,
        };

        // Capture the originating surface before `metadata` is moved into the
        // request — used below to decide cross-surface reply fan-out.
        let current_channel = metadata.get("channel_id").cloned().unwrap_or_default();

        // Convert RPC attachments (base64 payload from chat.send / agent.run)
        // into channel attachments so the engine's media processor sees them.
        // Previously this was hardcoded to `vec![]`, silently dropping every
        // attachment sent from the Panel. Undecodable entries are skipped with
        // a warning rather than failing the whole run.
        let attachments: Vec<crate::gateway::channel::Attachment> = params
            .attachments
            .iter()
            .filter_map(|a| match BASE64.decode(a.data.as_bytes()) {
                Ok(bytes) => Some(crate::gateway::channel::Attachment {
                    id: uuid::Uuid::new_v4().to_string(),
                    mime_type: a.mime_type.clone(),
                    filename: Some(a.name.clone()),
                    size: Some(bytes.len() as u64),
                    url: None,
                    path: None,
                    data: Some(bytes),
                }),
                Err(e) => {
                    error!(name = %a.name, error = %e, "Dropping attachment with invalid base64 data");
                    None
                }
            })
            .collect();

        // Voice turns may pin a low-TTFT model (config `[voice]
        // llm_provider/llm_model`) so the spoken reply starts faster — the
        // same pin the channel voice path applies. An explicit per-turn
        // override from the model picker always wins.
        let model_override = match (params.model_override, params.voice_input, &self.app_config) {
            (Some(o), _, _) => Some(o),
            (None, true, Some(cfg)) => {
                let cfg = cfg.read().await;
                crate::gateway::model_override::ModelOverride::from_voice(
                    &cfg.voice_local.llm_provider,
                    &cfg.voice_local.llm_model,
                )
            }
            (None, _, _) => None,
        };

        // Round-2 E3: a picker selection of the "moa" pseudo-provider (from
        // `providers.catalog`) arms session MoA instead of riding as a
        // per-turn override — see `apply_moa_selector_semantics`.
        let model_override = apply_moa_selector_semantics(&session_key_str, model_override);

        let request = RunRequest {
            run_id: run_id.clone(),
            input: params.input.clone(),
            session_key: session_key.clone(),
            timeout_secs: None,
            metadata,
            attachments,
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override,
            max_iterations_override: None,
            model_override,
        };

        // Primary emitter: streams to the Panel via the gateway event bus.
        // Read the global output_mode fresh per run so the live
        // typewriter/instant switch reaches Panel runs — mirroring the inbound
        // channel path. Previously this hardcoded `GatewayEventEmitter::new`
        // (always typewriter), so the Panel — the primary surface — ignored the
        // global toggle entirely; instant mode only worked on chat channels.
        let output_mode = self.resolved_output_mode().await;
        let base_emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(
            GatewayEventEmitter::with_output_mode(self.event_bus.clone(), output_mode),
        );

        // Sub-gap (b): if this gateway/Panel run continues a session that
        // originated on an *external* channel (Telegram, ...), wrap the emitter
        // so the final reply is also delivered back to that origin channel —
        // keeping the two surfaces in sync. Inbound channel runs never reach
        // this handler (they deliver via ReplyEmitter), so there is no double
        // delivery; the surface check below skips same-channel continuations.
        let emitter: Arc<dyn EventEmitter + Send + Sync> = match (
            agent.origin_route(&session_key).await,
            crate::gateway::event_emitter::origin_fanout::channel_registry(),
        ) {
            (Some((origin_channel, origin_conversation)), Some(registry))
                if origin_channel != current_channel =>
            {
                Arc::new(
                    crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                        base_emitter,
                        registry,
                        origin_channel,
                        origin_conversation,
                    ),
                )
            }
            _ => base_emitter,
        };

        let execution_adapter = self.execution_adapter.clone();
        let active_runs = self.active_runs.clone();
        let run_id_for_spawn = run_id.clone();

        tokio::spawn(async move {
            let result = execution_adapter.execute(request, agent, emitter).await;

            let mut runs = active_runs.write().await;
            if let Some(run) = runs.get_mut(&run_id_for_spawn) {
                run.status = match result {
                    Ok(()) => RunStatus::Completed,
                    Err(e) => RunStatus::Failed(e.to_string()),
                };
            }
        });

        Ok(AgentRunResult {
            run_id,
            session_key: session_key_str,
            accepted_at,
        })
    }

    /// Get status of a run
    pub async fn get_run_status(&self, run_id: &str) -> Option<RunState> {
        self.active_runs.read().await.get(run_id).cloned()
    }

    /// Cancel an active run
    pub async fn cancel_run(&self, run_id: &str) -> bool {
        // Forward to the execution engine FIRST: firing the run's
        // CancellationToken is the only thing that actually interrupts the
        // in-flight Think→Act loop (LLM call, tool execution, and between
        // iterations). Updating local status alone never reaches the running
        // task, which is why the chat-window Stop button used to do nothing.
        let signalled = self.execution_adapter.cancel(run_id).await.is_ok();

        // Reflect the request in our local bookkeeping so status queries
        // report Cancelled even if the run finished between the signal and now.
        let mut runs = self.active_runs.write().await;
        let was_running = matches!(
            runs.get(run_id).map(|r| &r.status),
            Some(RunStatus::Running)
        );
        if was_running {
            if let Some(run) = runs.get_mut(run_id) {
                run.status = RunStatus::Cancelled;
            }
        }

        signalled || was_running
    }

    /// List active runs
    pub async fn list_runs(&self) -> Vec<RunState> {
        self.active_runs.read().await.values().cloned().collect()
    }

    /// Snapshot of the execution engine's run-concurrency slot usage, for
    /// `gateway.metrics.run_concurrency` (Task 8, audit 3.4).
    pub fn concurrency_snapshot(&self) -> crate::gateway::execution_engine::ConcurrencySnapshot {
        self.execution_adapter.concurrency_snapshot()
    }

    /// Session keys with a run currently in flight (authoritative in-memory
    /// admission gate), for `gateway.metrics.run_concurrency` — lets the Panel
    /// paint per-session running indicators regardless of which interface
    /// started the run.
    #[must_use]
    pub fn running_sessions(&self) -> Vec<String> {
        self.execution_adapter.running_sessions()
    }
}

/// Round-2 E3: a picker selection of the "moa" pseudo-provider arms session
/// MoA (sticky) instead of riding as a per-turn model override. Returns the
/// override that should continue down the run path (None when consumed).
/// An explicit NON-moa override clears any MoA sticky — the selector is one
/// exclusive slot. No override touches nothing (bare sends must not disturb
/// an armed MoA session).
fn apply_moa_selector_semantics(
    session_key: &str,
    model_override: Option<crate::gateway::model_override::ModelOverride>,
) -> Option<crate::gateway::model_override::ModelOverride> {
    use crate::gateway::model_override::ModelOverride;
    match model_override {
        Some(ModelOverride::Qualified { provider, model }) if provider == "moa" => {
            crate::providers::session_moa_handle::set_session_moa(
                session_key,
                Some(model),
                false,
            );
            crate::providers::session_model_handle::clear_session_model(session_key);
            None
        }
        Some(other) => {
            crate::providers::session_moa_handle::clear_session_moa(session_key);
            Some(other)
        }
        None => None,
    }
}

/// Handle agent.run RPC request
pub async fn handle_run(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: AgentRunParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate input
    if params.input.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Input cannot be empty");
    }

    // Start the run
    match run_manager.start_run(params).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Handle agent.status RPC request
/// Parameters for agent.status / agent.cancel
#[derive(Debug, Deserialize)]
struct RunIdParams {
    run_id: String,
}

pub async fn handle_status(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    let params: RunIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match run_manager.get_run_status(&params.run_id).await {
        Some(state) => {
            let status_str = match &state.status {
                RunStatus::Running => "running",
                RunStatus::Completed => "completed",
                RunStatus::Failed(_) => "failed",
                RunStatus::Cancelled => "cancelled",
            };
            JsonRpcResponse::success(
                request.id,
                json!({
                    "run_id": state.run_id,
                    "session_key": state.session_key.to_key_string(),
                    "status": status_str,
                    "elapsed_ms": state.started_at.elapsed().as_millis() as u64,
                }),
            )
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Run not found"),
    }
}

/// Handle agent.cancel RPC request
pub async fn handle_cancel(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    let params: RunIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let cancelled = run_manager.cancel_run(&params.run_id).await;
    JsonRpcResponse::success(
        request.id,
        json!({
            "run_id": params.run_id,
            "cancelled": cancelled,
        }),
    )
}

/// Handle agents.list RPC request
pub async fn handle_list(request: JsonRpcRequest, router: Arc<AgentRouter>) -> JsonRpcResponse {
    let agents = router.list_agents().await;
    JsonRpcResponse::success(
        request.id,
        json!({
            "agents": agents,
            "default": router.default_agent(),
        }),
    )
}

// ============================================================================
// Extended Agent Handlers (for remove-ffi migration)
// ============================================================================

/// Parameters for agent.confirmPlan
#[derive(Debug, Deserialize)]
pub(crate) struct ConfirmPlanParams {
    /// Plan ID to confirm/reject
    pub plan_id: String,
    /// Whether to confirm (true) or reject (false)
    pub confirmed: bool,
}

/// Handle agent.confirmPlan RPC request
///
/// Confirms or rejects a task plan that requires user approval.
pub async fn handle_confirm_plan(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ConfirmPlanParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Forward to active agent instance
    info!(
        plan_id = %params.plan_id,
        confirmed = params.confirmed,
        "Plan confirmation received"
    );

    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Parameters for agent.respondToInput
#[derive(Debug, Deserialize)]
pub(crate) struct RespondToInputParams {
    /// Request ID for the user input request
    pub request_id: String,
    /// User's response
    pub response: String,
}

/// Handle agent.respondToInput RPC request
///
/// Responds to a user input request from the agent.
pub async fn handle_respond_to_input(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: RespondToInputParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // TODO: Forward to active agent instance
    info!(
        request_id = %params.request_id,
        response_len = params.response.len(),
        "User input response received"
    );

    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// Parameters for agent.generateTitle
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct GenerateTitleParams {
    /// User's input message
    pub user_input: String,
    /// AI's response
    pub ai_response: String,
}

/// Handle agent.generateTitle RPC request
///
/// Generates a title for a conversation based on the first exchange.
pub async fn handle_generate_title(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: GenerateTitleParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Generate a simple title from user input
    // TODO: Use AI to generate a better title
    let title = if params.user_input.chars().count() > 50 {
        let truncated: String = params.user_input.chars().take(47).collect();
        format!("{truncated}...")
    } else {
        params.user_input.clone()
    };

    JsonRpcResponse::success(request.id, json!({ "title": title }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_engine::{
        ExecutionError, RunRequest, RunState, RunStatus as EngineRunStatus,
    };
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::SessionStore;
    use async_trait::async_trait;
    use chrono::Utc;

    /// Build a registry containing a "main" AgentInstance backed by a tempdir
    /// FileSessionStore. Holds onto the TempDir so callers can keep it alive
    /// for the duration of the test (dropping it tears down the workspace).
    async fn registry_with_main_agent() -> (Arc<AgentRegistry>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let config = AgentInstanceConfig {
            agent_id: "main".into(),
            workspace: tmp.path().join("workspace"),
            agent_dir: tmp.path().join("agent"),
            ..AgentInstanceConfig::default()
        };
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: tmp.path().join("sessions"),
            ..FileSessionStoreConfig::default()
        })
        .expect("FileSessionStore::new");
        let session_store: Arc<dyn SessionStore> = Arc::new(store);
        let instance = AgentInstance::new(config, session_store).expect("AgentInstance::new");
        let registry = Arc::new(AgentRegistry::new());
        registry.register(instance).await;
        (registry, tmp)
    }

    struct MockExecutionAdapter;

    #[async_trait]
    impl ExecutionAdapter for MockExecutionAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            Err(ExecutionError::RunNotFound(run_id.to_string()))
        }

        async fn get_status(&self, run_id: &str) -> Option<EngineRunStatus> {
            Some(EngineRunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
                steps_completed: 0,
                current_tool: None,
            })
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    /// Execution adapter that records every `cancel(run_id)` it receives, so a
    /// test can assert the run manager actually forwards cancellation to the
    /// execution engine (the only thing that fires the run's CancellationToken).
    #[derive(Default)]
    struct RecordingExecutionAdapter {
        cancelled: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ExecutionAdapter for RecordingExecutionAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            self.cancelled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(run_id.to_string());
            Ok(())
        }

        async fn get_status(&self, run_id: &str) -> Option<EngineRunStatus> {
            Some(EngineRunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
                steps_completed: 0,
                current_tool: None,
            })
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    #[tokio::test]
    async fn test_agent_run_manager() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "Hello, world!".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            voice_input: false,
        };

        let result = manager.start_run(params).await.unwrap();
        assert!(!result.run_id.is_empty());
        assert!(result.session_key.starts_with("agent:main:"));
    }

    // The Panel voice loop sends chat.send { voice_input: true }. start_run
    // must record that in the session voice-mode registry BEFORE the run
    // spawns (VoiceModeLayer reads it at prompt assembly), and a following
    // typed turn must clear it — otherwise the spoken-reply style leaks into
    // text chat.
    #[tokio::test]
    async fn voice_input_round_trips_through_session_mode_registry() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let voice_params = AgentRunParams {
            input: "把窗帘拉上".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            voice_input: true,
        };
        let result = manager.start_run(voice_params).await.unwrap();
        assert_eq!(
            crate::gateway::voice::session_mode::get(&result.session_key),
            Some(true),
            "a voice turn must mark the session voice-active with transcribed input"
        );

        // Same session, typed turn → the flag clears.
        let typed_params = AgentRunParams {
            input: "thanks".to_string(),
            session_key: Some(result.session_key.clone()),
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            voice_input: false,
        };
        let result2 = manager.start_run(typed_params).await.unwrap();
        assert_eq!(result2.session_key, result.session_key);
        assert_eq!(
            crate::gateway::voice::session_mode::get(&result.session_key),
            None,
            "a typed turn must clear the voice-mode flag"
        );
    }

    // Regression: a Panel run must stamp `metadata["platform"]="webchat"` so the
    // run-loop derives a WebRich (interactive) InteractionManifest. Without the
    // tag the manifest falls back to `Background`, whose `SilentReply` capability
    // makes `ProtocolTokensLayer` teach `ALEPH_SILENT_COMPLETE`; the model then
    // emits that literal token on a silent completion and it leaks into the chat
    // bubble. This captures the RunRequest the handler hands to the execution
    // adapter and asserts the platform tag is present.
    #[tokio::test]
    async fn panel_run_tags_webchat_platform() {
        struct CapturingAdapter {
            platform: Arc<std::sync::Mutex<Option<Option<String>>>>,
        }
        #[async_trait]
        impl ExecutionAdapter for CapturingAdapter {
            async fn execute(
                &self,
                request: RunRequest,
                _agent: Arc<AgentInstance>,
                _emitter: Arc<dyn EventEmitter + Send + Sync>,
            ) -> Result<(), ExecutionError> {
                *self.platform.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(request.metadata.get("platform").cloned());
                Ok(())
            }
            async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
                Err(ExecutionError::RunNotFound(run_id.to_string()))
            }
            async fn get_status(&self, _run_id: &str) -> Option<EngineRunStatus> {
                None
            }
            async fn active_run_count(&self) -> usize {
                0
            }
        }

        let captured: Arc<std::sync::Mutex<Option<Option<String>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let adapter = Arc::new(CapturingAdapter {
            platform: captured.clone(),
        });
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let manager = AgentRunManager::new(
            Arc::new(AgentRouter::new()),
            Arc::new(GatewayEventBus::new()),
            agent_registry,
            adapter,
        );
        let params = AgentRunParams {
            input: "hi".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            voice_input: false,
        };
        manager.start_run(params).await.expect("start_run");

        // Execution is spawned (`tokio::spawn`), so poll until the adapter
        // records the RunRequest the handler built.
        let mut captured_platform = None;
        for _ in 0..200 {
            if let Some(p) = captured.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                captured_platform = Some(p);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            captured_platform,
            Some(Some("webchat".to_string())),
            "Panel run must tag platform=webchat (else Background paradigm → ALEPH_SILENT_COMPLETE leaks)"
        );
    }

    // Regression: Panel runs must honor the global `behavior.output_mode`
    // toggle fresh per run (the same key chat channels read). The bug this
    // guards: `start_run` used to hardcode `GatewayEventEmitter::new`
    // (typewriter), so flipping the switch to "instant" had no effect on the
    // Panel — the primary surface — even though every chat channel obeyed it.
    #[tokio::test]
    async fn panel_run_resolves_global_output_mode() {
        fn manager(cfg: Option<crate::Config>) -> AgentRunManager {
            let m = AgentRunManager::new(
                Arc::new(AgentRouter::new()),
                Arc::new(GatewayEventBus::new()),
                Arc::new(AgentRegistry::new()),
                Arc::new(MockExecutionAdapter),
            );
            match cfg {
                Some(c) => m.with_app_config(Arc::new(RwLock::new(c))),
                None => m,
            }
        }

        // Absent config (tests / host-only) → typewriter, matching the inbound
        // channel fallback in `inbound_router::executor`.
        assert!(matches!(
            manager(None).resolved_output_mode().await,
            OutputMode::Typewriter
        ));

        // Global switch = "instant" → Panel run resolves to instant.
        let instant: crate::Config =
            serde_json::from_value(json!({ "behavior": { "output_mode": "instant" } })).unwrap();
        assert!(matches!(
            manager(Some(instant)).resolved_output_mode().await,
            OutputMode::Instant
        ));

        // Global switch = "typewriter" → typewriter.
        let typewriter: crate::Config =
            serde_json::from_value(json!({ "behavior": { "output_mode": "typewriter" } })).unwrap();
        assert!(matches!(
            manager(Some(typewriter)).resolved_output_mode().await,
            OutputMode::Typewriter
        ));
    }

    #[tokio::test]
    async fn test_run_status() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "Test".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            voice_input: false,
        };

        let result = manager.start_run(params).await.unwrap();

        // Should be able to get status
        let status = manager.get_run_status(&result.run_id).await;
        assert!(status.is_some());
    }

    /// Layer-2 gate: a chat-tier ("guest") connection may converse but must not
    /// pick an arbitrary working directory — even a valid absolute dir is denied.
    #[tokio::test]
    async fn chat_tier_caller_cannot_choose_project_root() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "work over here".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: Some(std::env::temp_dir().display().to_string()),
            model_override: None,
            voice_input: false,
        };

        let result = crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("guest".to_string()), manager.start_run(params))
            .await;
        let err = result.expect_err("chat-tier project_root override must be rejected");
        assert!(err.contains("config-tier"), "unexpected error: {err}");
    }

    /// Config-tier ("operator") and absent-role (trusted local) callers may
    /// choose a working directory; absent role exercises the existing default.
    #[tokio::test]
    async fn config_tier_caller_may_choose_project_root() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "work over here".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: Some(std::env::temp_dir().display().to_string()),
            model_override: None,
            voice_input: false,
        };

        let result = crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), manager.start_run(params))
            .await;
        assert!(
            result.is_ok(),
            "operator project_root override must succeed: {result:?}"
        );
    }

    /// Desktop App: a chat-tier ("guest") connection over loopback may choose a
    /// project_root — the local Panel is the operator. This is the additive
    /// loopback allowance on top of the config-tier gate.
    #[tokio::test]
    async fn loopback_chat_tier_caller_may_choose_project_root() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "work over here".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: Some(std::env::temp_dir().display().to_string()),
            model_override: None,
            voice_input: false,
        };

        // chat-tier role but loopback connection → allowed.
        let result = crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                Some("guest".to_string()),
                crate::gateway::caller_identity::CALLER_IS_LOOPBACK
                    .scope(true, manager.start_run(params)),
            )
            .await;
        assert!(
            result.is_ok(),
            "loopback chat-tier project_root override must succeed: {result:?}"
        );
    }

    /// Regression: aborting a run must forward the cancellation to the
    /// execution engine. Flipping only the local status is a no-op for the
    /// in-flight Think→Act loop — the engine's CancellationToken is the sole
    /// mechanism that actually stops it, so `cancel_run` MUST call
    /// `ExecutionAdapter::cancel`.
    #[tokio::test]
    async fn cancel_run_forwards_to_execution_adapter() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let adapter = Arc::new(RecordingExecutionAdapter::default());
        let execution_adapter: Arc<dyn ExecutionAdapter> = adapter.clone();
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        manager.cancel_run("run-under-test").await;

        let cancelled = adapter.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            &*cancelled,
            &["run-under-test".to_string()],
            "cancel_run must forward the run_id to ExecutionAdapter::cancel"
        );
    }

    /// Round-2 E3: selecting the "moa" pseudo-provider row in the picker
    /// arms sticky session MoA and is consumed (no override rides the run
    /// path); an explicit non-moa override clears the sticky instead; a bare
    /// send (no override) leaves an armed session alone.
    #[test]
    fn moa_override_arms_session_and_is_consumed() {
        use crate::gateway::model_override::ModelOverride;
        let key = "test:moa:selector";
        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified {
                provider: "moa".into(),
                model: "deep".into(),
            }),
        );
        assert!(out.is_none());
        let pref = crate::providers::session_moa_handle::get_session_moa(key).unwrap();
        assert_eq!(pref.preset.as_deref(), Some("deep"));
        assert!(!pref.one_shot);

        // Non-moa override clears the sticky.
        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified {
                provider: "openai".into(),
                model: "gpt-5".into(),
            }),
        );
        assert!(out.is_some());
        assert!(crate::providers::session_moa_handle::get_session_moa(key).is_none());

        // No override leaves state alone.
        crate::providers::session_moa_handle::set_session_moa(key, None, false);
        assert!(apply_moa_selector_semantics(key, None).is_none());
        assert!(crate::providers::session_moa_handle::get_session_moa(key).is_some());
        crate::providers::session_moa_handle::clear_session_moa(key);
    }
}
