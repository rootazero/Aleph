//! Server initialization helpers for Aleph Gateway
//!
//! This module contains helper functions for server initialization,
//! including WebChat serving and agent run handling.

use std::net::SocketAddr;
use std::path::PathBuf;

use alephcore::sync_primitives::Arc;

use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::{
    AgentRegistry, EventEmitter, ExecutionEngine, GatewayEventEmitter, StreamEvent,
};

/// Serve WebChat static files
pub async fn serve_webchat(
    addr: SocketAddr,
    static_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::Router;
    use tower_http::services::{ServeDir, ServeFile};

    tracing::info!("Starting WebChat server on http://{}", addr);

    // Create fallback for SPA routing
    let index_path = static_dir.join("index.html");
    let serve_dir = ServeDir::new(&static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(&index_path));

    let allowed_origins = tower_http::cors::AllowOrigin::predicate(|origin, _| {
        // `origin` is the raw `Origin` header value, not a parsed URI — parse it
        // before inspecting scheme/host. A bare prefix match would otherwise
        // accept hostile origins like `http://127.evil.com`.
        let Ok(origin_str) = origin.to_str() else {
            return false;
        };
        let Ok(uri) = origin_str.parse::<axum::http::Uri>() else {
            return false;
        };
        let scheme = uri.scheme_str().unwrap_or("");
        if scheme != "http" && scheme != "https" {
            return false;
        }
        let host = uri.host().unwrap_or("");
        matches!(host, "127.0.0.1" | "localhost" | "[::1]") || host.starts_with("127.0.0.")
    });
    let app = Router::new().fallback_service(serve_dir).layer(
        tower_http::cors::CorsLayer::new()
            .allow_origin(allowed_origins)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::HEAD,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::ACCEPT,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
            ]),
    );

    // Create listener
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Serve
    axum::serve(listener, app).await?;

    Ok(())
}

/// Handle agent.run with real ExecutionEngine
pub async fn handle_run_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    _workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
) -> alephcore::gateway::JsonRpcResponse
where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
{
    use alephcore::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS};
    use alephcore::gateway::RunRequest;
    use serde::{Deserialize, Serialize};
    use serde_json::{json, Value};

    // Deserialized from JSON-RPC params; fields read via serde
    #[derive(Debug, Clone, Deserialize)]
    struct AgentRunParams {
        pub input: String,
        #[serde(default)]
        pub session_key: Option<String>,
        #[serde(default)]
        pub channel: Option<String>,
        #[serde(default)]
        pub peer_id: Option<String>,
        #[serde(default = "default_stream")]
        #[allow(dead_code)] // deserialized request param, not yet wired
        pub stream: bool,
        /// Optional absolute project root for per-run `workspace_override`.
        #[serde(default)]
        pub project_root: Option<String>,
    }

    fn default_stream() -> bool {
        true
    }

    /// Result of agent.run request
    #[derive(Debug, Clone, Serialize)]
    struct AgentRunResult {
        pub run_id: String,
        pub session_key: String,
        pub accepted_at: String,
    }

    // Parse params
    let params: AgentRunParams = match request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map)) {
            Ok(p) => p,
            Err(e) => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        _ => {
            return alephcore::gateway::JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate input
    if params.input.trim().is_empty() {
        return alephcore::gateway::JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Input cannot be empty",
        );
    }

    // Generate run ID
    let run_id = uuid::Uuid::new_v4().to_string();

    // Resolve session key
    let session_key = router
        .route(
            params.session_key.as_deref(),
            params.channel.as_deref(),
            params.peer_id.as_deref(),
            None, // agent_id: legacy path, no explicit agent
        )
        .await;

    let session_key_str = session_key.to_key_string();
    let accepted_at = chrono::Utc::now().to_rfc3339();

    // Resolve agent from session_key (which encodes the correct agent_id)
    let resolved_agent_id = session_key.agent_id().to_string();

    let agent = {
        let agent_opt = agent_registry.get(&resolved_agent_id).await;
        match agent_opt.or(agent_registry.get_default().await) {
            Some(a) => a,
            None => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    "No default agent available",
                );
            }
        }
    };

    let channel_id = params.channel.as_deref().unwrap_or("panel");
    let peer_id = params.peer_id.as_deref().unwrap_or("local");

    // Create emitter for streaming events, respecting output_mode config
    let output_mode = {
        let cfg = app_config.read().await;
        let behavior = cfg.behavior.as_ref();
        let mode_str = behavior
            .map(|b| b.output_mode.as_str())
            .unwrap_or("typewriter");
        alephcore::gateway::OutputMode::from_config(mode_str)
    };
    let emitter = Arc::new(GatewayEventEmitter::with_output_mode(
        event_bus.clone(),
        output_mode,
    ));

    // Create run request with channel/peer metadata for agent management tools
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("channel_id".to_string(), channel_id.to_string());
    metadata.insert("sender_id".to_string(), peer_id.to_string());

    let workspace_override = match params.project_root.as_deref() {
        Some(raw) => {
            let path = std::path::PathBuf::from(raw);
            if !path.is_absolute() {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("project_root must be absolute: {raw}"),
                );
            }
            if !path.is_dir() {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("project_root is not a directory: {raw}"),
                );
            }
            metadata.insert("project_root".to_string(), path.display().to_string());
            Some(path)
        }
        None => None,
    };

    let run_request = RunRequest {
        run_id: run_id.clone(),
        input: params.input.clone(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override,
        max_iterations_override: None,
        model_override: None,
    };

    // Spawn execution task
    let engine_clone = engine.clone();
    let emitter_clone = emitter.clone();
    let run_id_clone = run_id.clone();
    tokio::spawn(async move {
        match engine_clone
            .execute(run_request, agent, emitter_clone.clone())
            .await
        {
            Ok(()) => {
                tracing::info!(run_id = %run_id_clone, "Agent run completed successfully");
            }
            Err(e) => {
                tracing::error!(run_id = %run_id_clone, error = %e, "Agent run failed");
                if let Err(emit_err) = emitter_clone
                    .emit(StreamEvent::RunError {
                        run_id: run_id_clone.clone(),
                        seq: 0,
                        error: e.to_string(),
                        error_code: Some("EXECUTION_FAILED".to_string()),
                    })
                    .await
                {
                    tracing::warn!("Failed to emit run error event: {}", emit_err);
                }
            }
        }
    });

    // Return immediate response
    let result = AgentRunResult {
        run_id,
        session_key: session_key_str,
        accepted_at,
    };

    alephcore::gateway::JsonRpcResponse::success(request.id, json!(result))
}

/// Handle chat.send with real ExecutionEngine
///
/// Same as `handle_run_with_engine` but accepts `chat.send` param format
/// (message instead of input) and returns chat-friendly response.
#[allow(clippy::too_many_arguments)]
pub async fn handle_chat_send_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    _workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
    _provider_registry: Arc<P>,
    _session_manager: Arc<dyn alephcore::gateway::session_store::SessionStore>,
    command_parser: Option<Arc<alephcore::command::CommandParser>>,
) -> alephcore::gateway::JsonRpcResponse
where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
{
    use alephcore::gateway::handlers::chat::SendParams;
    use alephcore::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS};
    use alephcore::gateway::RunRequest;
    use serde::Serialize;
    use serde_json::{json, Value};

    #[derive(Debug, Clone, Serialize)]
    struct ChatSendResult {
        pub run_id: String,
        pub session_key: String,
        pub streaming: bool,
    }

    // Parse params
    let params: SendParams = match request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map)) {
            Ok(p) => p,
            Err(e) => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        _ => {
            return alephcore::gateway::JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate message
    if params.message.trim().is_empty() {
        return alephcore::gateway::JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Message cannot be empty",
        );
    }

    // Generate run ID
    let run_id = uuid::Uuid::new_v4().to_string();

    // Resolve session key (with explicit agent_id from Panel if provided)
    let session_key = router
        .route(
            params.session_key.as_deref(),
            params.channel.as_deref(),
            None,
            params.agent_id.as_deref(),
        )
        .await;

    let session_key_str = session_key.to_key_string();

    // Resolve agent from session_key (which now encodes the correct agent_id)
    let resolved_agent_id = session_key.agent_id().to_string();

    let agent = {
        let agent_opt = agent_registry.get(&resolved_agent_id).await;
        match agent_opt.or(agent_registry.get_default().await) {
            Some(a) => a,
            None => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    "No default agent available",
                );
            }
        }
    };

    let channel_id = params.channel.as_deref().unwrap_or("panel");
    let peer_id = "local"; // Panel doesn't have per-user peer IDs

    // Create emitter for streaming events, respecting output_mode config
    let output_mode = {
        let cfg = app_config.read().await;
        let behavior = cfg.behavior.as_ref();
        let mode_str = behavior
            .map(|b| b.output_mode.as_str())
            .unwrap_or("typewriter");
        alephcore::gateway::OutputMode::from_config(mode_str)
    };
    let emitter = Arc::new(GatewayEventEmitter::with_output_mode(
        event_bus.clone(),
        output_mode,
    ));

    // Create run request with channel/peer metadata for agent management tools
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("channel_id".to_string(), channel_id.to_string());
    metadata.insert("sender_id".to_string(), peer_id.to_string());

    // Inject user locale for downstream i18n
    {
        let cfg = app_config.read().await;
        let lang = cfg.general.language.as_deref().unwrap_or("zh");
        metadata.insert("locale".to_string(), lang.to_string());
    }

    // Resolve optional project_root → workspace_override. Rejected if the
    // path is not absolute or not an existing directory so downstream code
    // can trust the override.
    let project_root_for_run = match params.project_root.as_deref() {
        Some(raw) => {
            let path = std::path::PathBuf::from(raw);
            if !path.is_absolute() {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("project_root must be absolute: {raw}"),
                );
            }
            if !path.is_dir() {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("project_root is not a directory: {raw}"),
                );
            }
            metadata.insert("project_root".to_string(), path.display().to_string());
            Some(path)
        }
        None => None,
    };

    // Slash command detection: resolve via CommandParser and emit the
    // source-aware mode JSON (preserves Skill instructions / Custom system
    // prompt / MCP server name so the fast path can act on them).
    if params.message.trim().starts_with('/') {
        if let Some(ref parser) = command_parser {
            let slash_text = params.message.trim();
            if let Some(parsed) = parser.parse_async(slash_text).await {
                if let Some(mode_json) =
                    alephcore::gateway::inbound_router::serialize_parsed_command(&parsed)
                {
                    tracing::info!(
                        "[chat.send] Slash command resolved: name={}, args={:?}",
                        parsed.command_name,
                        parsed.arguments
                    );
                    metadata.insert(
                        alephcore::gateway::inbound_router::SLASH_COMMAND_MODE_KEY.to_string(),
                        mode_json,
                    );
                }
            }
        }
    }

    let run_request = RunRequest {
        run_id: run_id.clone(),
        input: params.message.clone(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: project_root_for_run.clone(),
        max_iterations_override: None,
        model_override: None,
    };

    // Spawn execution task
    let engine_clone = engine.clone();
    let emitter_clone = emitter.clone();
    let run_id_clone = run_id.clone();
    tokio::spawn(async move {
        match engine_clone
            .execute(run_request, agent, emitter_clone.clone())
            .await
        {
            Ok(()) => {
                tracing::info!(run_id = %run_id_clone, "Chat run completed successfully");
            }
            Err(e) => {
                tracing::error!(run_id = %run_id_clone, error = %e, "Chat run failed");
                if let Err(emit_err) = emitter_clone
                    .emit(StreamEvent::RunError {
                        run_id: run_id_clone.clone(),
                        seq: 0,
                        error: e.to_string(),
                        error_code: Some("EXECUTION_FAILED".to_string()),
                    })
                    .await
                {
                    tracing::warn!("Failed to emit chat run error event: {}", emit_err);
                }
            }
        }
    });

    // Auto-topic generation is now handled by ExecutionEngine on first message

    // Return immediate response
    let result = ChatSendResult {
        run_id,
        session_key: session_key_str,
        streaming: params.stream,
    };

    alephcore::gateway::JsonRpcResponse::success(request.id, json!(result))
}
