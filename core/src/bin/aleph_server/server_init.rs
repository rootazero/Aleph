//! Server initialization helpers for Aleph Gateway
//!
//! This module contains helper functions for server initialization,
//! including WebChat serving and agent run handling.

use std::net::SocketAddr;
use std::path::PathBuf;

use std::sync::Arc;

use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::{
    ExecutionEngine, GatewayEventEmitter, AgentRegistry,
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

    // Build router with CORS headers for development
    let app = Router::new()
        .fallback_service(serve_dir)
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
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
    #[allow(dead_code)] // Deserialized from JSON-RPC request params
    struct AgentRunParams {
        pub input: String,
        #[serde(default)]
        pub session_key: Option<String>,
        #[serde(default)]
        pub channel: Option<String>,
        #[serde(default)]
        pub peer_id: Option<String>,
        #[serde(default = "default_stream")]
        pub stream: bool,
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
        )
        .await;

    let session_key_str = session_key.to_key_string();
    let accepted_at = chrono::Utc::now().to_rfc3339();

    // Get default agent
    let agent = match agent_registry.get_default().await {
        Some(a) => a,
        None => {
            return alephcore::gateway::JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "No default agent available",
            );
        }
    };

    // Create emitter for streaming events
    let emitter = Arc::new(GatewayEventEmitter::new(event_bus.clone()));

    // Create run request
    let run_request = RunRequest {
        run_id: run_id.clone(),
        input: params.input.clone(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata: std::collections::HashMap::new(),
    };

    // Spawn execution task
    let engine_clone = engine.clone();
    let emitter_clone = emitter.clone();
    let run_id_clone = run_id.clone();
    tokio::spawn(async move {
        match engine_clone
            .execute(run_request, agent, emitter_clone)
            .await
        {
            Ok(()) => {
                tracing::info!(run_id = %run_id_clone, "Agent run completed successfully");
            }
            Err(e) => {
                tracing::error!(run_id = %run_id_clone, error = %e, "Agent run failed");
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
pub async fn handle_chat_send_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
) -> alephcore::gateway::JsonRpcResponse
where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
{
    use alephcore::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS};
    use alephcore::gateway::RunRequest;
    use alephcore::gateway::handlers::chat::SendParams;
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

    // Resolve session key
    let session_key = router
        .route(
            params.session_key.as_deref(),
            params.channel.as_deref(),
            None,
        )
        .await;

    let session_key_str = session_key.to_key_string();

    // Get default agent
    let agent = match agent_registry.get_default().await {
        Some(a) => a,
        None => {
            return alephcore::gateway::JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "No default agent available",
            );
        }
    };

    // Create emitter for streaming events
    let emitter = Arc::new(GatewayEventEmitter::new(event_bus.clone()));

    // Create run request
    let run_request = RunRequest {
        run_id: run_id.clone(),
        input: params.message.clone(),
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata: std::collections::HashMap::new(),
    };

    // Spawn execution task
    let engine_clone = engine.clone();
    let emitter_clone = emitter.clone();
    let run_id_clone = run_id.clone();
    tokio::spawn(async move {
        match engine_clone
            .execute(run_request, agent, emitter_clone)
            .await
        {
            Ok(()) => {
                tracing::info!(run_id = %run_id_clone, "Chat run completed successfully");
            }
            Err(e) => {
                tracing::error!(run_id = %run_id_clone, error = %e, "Chat run failed");
            }
        }
    });

    // Return immediate response
    let result = ChatSendResult {
        run_id,
        session_key: session_key_str,
        streaming: params.stream,
    };

    alephcore::gateway::JsonRpcResponse::success(request.id, json!(result))
}
