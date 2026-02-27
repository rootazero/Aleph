//! Agent Handlers
//!
//! RPC handlers for agent operations: run, wait, cancel, status.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::super::event_bus::GatewayEventBus;
use super::super::event_emitter::{
    EventEmitter, GatewayEventEmitter, RunSummary, StreamEvent,
};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::super::router::{AgentRouter, SessionKey};

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
}

impl AgentRunManager {
    pub fn new(router: Arc<AgentRouter>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            router,
            event_bus,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
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
                let notification =
                    super::super::protocol::JsonRpcRequest::notification("stream.run_accepted", Some(event_value));
                if let Ok(json) = serde_json::to_string(&notification) {
                    self.event_bus.publish(json);
                }
            }
        }

        // Spawn the actual agent execution (simulated for now)
        let event_bus = self.event_bus.clone();
        let active_runs = self.active_runs.clone();
        let run_id_clone = run_id.clone();
        let input = params.input.clone();

        tokio::spawn(async move {
            execute_agent_run(run_id_clone, input, event_bus, active_runs).await;
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
        let mut runs = self.active_runs.write().await;
        if let Some(run) = runs.get_mut(run_id) {
            if run.status == RunStatus::Running {
                run.status = RunStatus::Cancelled;
                return true;
            }
        }
        false
    }

    /// List active runs
    pub async fn list_runs(&self) -> Vec<RunState> {
        self.active_runs.read().await.values().cloned().collect()
    }
}

/// Execute an agent run (simulated implementation)
///
/// In a real implementation, this would call the actual agent loop.
/// For Phase 2, we simulate the execution with mock events.
async fn execute_agent_run(
    run_id: String,
    input: String,
    event_bus: Arc<GatewayEventBus>,
    active_runs: Arc<RwLock<HashMap<String, RunState>>>,
) {
    let emitter = GatewayEventEmitter::new(event_bus);
    let start_time = Instant::now();

    // Simulate reasoning
    emitter.emit_reasoning(&run_id, "Analyzing the request...", false).await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    emitter
        .emit_reasoning(&run_id, &format!("Processing input: {}", &input[..input.len().min(50)]), false)
        .await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    emitter.emit_reasoning(&run_id, "Formulating response...", true).await;

    // Simulate response
    let response = format!("Echo: {}", input);
    let chunk_size = 50;
    let chunks: Vec<&str> = response
        .as_bytes()
        .chunks(chunk_size)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();

    for (i, chunk) in chunks.iter().enumerate() {
        let is_final = i == chunks.len() - 1;
        emitter
            .emit_response_chunk(&run_id, chunk, i as u32, is_final)
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    // Complete the run
    let duration_ms = start_time.elapsed().as_millis() as u64;
    let summary = RunSummary {
        total_tokens: 100,
        tool_calls: 0,
        loops: 1,
        final_response: Some(response),
    };

    emitter.emit_run_complete(&run_id, summary, duration_ms).await;

    // Update run state
    {
        let mut runs = active_runs.write().await;
        if let Some(run) = runs.get_mut(&run_id) {
            run.status = RunStatus::Completed;
        }
    }

    debug!("Completed run {} in {}ms", run_id, duration_ms);
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
pub async fn handle_list(
    request: JsonRpcRequest,
    router: Arc<AgentRouter>,
) -> JsonRpcResponse {
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
pub struct ConfirmPlanParams {
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
pub struct RespondToInputParams {
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
pub struct GenerateTitleParams {
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
    let title = if params.user_input.len() > 50 {
        format!("{}...", &params.user_input[..47])
    } else {
        params.user_input.clone()
    };

    JsonRpcResponse::success(request.id, json!({ "title": title }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_run_manager() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = AgentRunManager::new(router, event_bus);

        let params = AgentRunParams {
            input: "Hello, world!".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
        };

        let result = manager.start_run(params).await.unwrap();
        assert!(!result.run_id.is_empty());
        assert!(result.session_key.starts_with("agent:main:"));
    }

    #[tokio::test]
    async fn test_run_status() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = AgentRunManager::new(router, event_bus);

        let params = AgentRunParams {
            input: "Test".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
        };

        let result = manager.start_run(params).await.unwrap();

        // Should be able to get status
        let status = manager.get_run_status(&result.run_id).await;
        assert!(status.is_some());
    }
}
