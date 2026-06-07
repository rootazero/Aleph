//! Agent Handlers
//!
//! RPC handlers for agent operations: run, wait, cancel, status.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::agent_instance::AgentRegistry;
use super::super::event_bus::GatewayEventBus;
use super::super::event_emitter::{EventEmitter, GatewayEventEmitter, StreamEvent};
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
}

fn default_stream() -> bool {
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

#[derive(Debug, Clone, PartialEq)]
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
                    run.status = RunStatus::Failed(format!("Agent not found: {}", agent_id));
                }
                return Err(format!("Agent not found: {}", agent_id));
            }
        };

        let mut metadata = HashMap::new();
        metadata.insert("channel_id".to_string(), params.channel.unwrap_or_default());
        metadata.insert("sender_id".to_string(), "websocket".to_string());
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

        let request = RunRequest {
            run_id: run_id.clone(),
            input: params.input.clone(),
            session_key: session_key.clone(),
            timeout_secs: None,
            metadata,
            attachments: vec![],
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override,
            max_iterations_override: None,
            model_override: params.model_override,
        };

        let emitter: Arc<dyn EventEmitter + Send + Sync> =
            Arc::new(GatewayEventEmitter::new(self.event_bus.clone()));

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
        format!("{}...", truncated)
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
        };

        let result = manager.start_run(params).await.unwrap();
        assert!(!result.run_id.is_empty());
        assert!(result.session_key.starts_with("agent:main:"));
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
        };

        let result = manager.start_run(params).await.unwrap();

        // Should be able to get status
        let status = manager.get_run_status(&result.run_id).await;
        assert!(status.is_some());
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
}
