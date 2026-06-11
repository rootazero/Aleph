//! Session creation handlers.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;

/// Handle session.create RPC request with database backend
///
/// Creates a new session and returns the session key and optional name.
pub async fn handle_create_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let name = request
        .params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Generate a unique session key based on timestamp
    let ts = chrono::Utc::now().timestamp_millis();
    let session_key_str = format!("session_{ts}");
    let session_key = SessionKey::Main {
        agent_id: name.clone().unwrap_or_else(|| "main".to_string()),
        main_key: session_key_str.clone(),
        epoch: 0,
    };

    match manager.get_or_create(&session_key).await {
        Ok(_meta) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key.to_key_string(),
                "name": name.unwrap_or(session_key_str),
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create session: {e}"),
        ),
    }
}

/// Handle sessions.new RPC request — close current session and create a new epoch
///
/// Params:
///   - session_key (required): current session key string
///   - topic (optional): topic for the closing session (if omitted, no topic is stored)
///
/// Returns:
///   - old_session_key: the closed session key
///   - new_session_key: the newly created session key (epoch incremented)
///   - topic: the topic stored (if any)
pub async fn handle_new_session_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    use crate::routing::session_key::SessionKey as RoutingKey;

    let session_key_str = match request
        .params
        .as_ref()
        .and_then(|p| p.get("session_key"))
        .and_then(|v| v.as_str())
    {
        Some(k) => k.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let topic = request
        .params
        .as_ref()
        .and_then(|p| p.get("topic"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Parse with legacy key for close_session compatibility
    let legacy_key = match SessionKey::from_key_string(&session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    // Close old session
    if let Err(e) = manager.close_session(&legacy_key, topic.as_deref()).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to close session: {e}"),
        );
    }

    // Parse with routing key for epoch support
    let routing_key = match RoutingKey::parse(&session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Cannot parse session key for epoch",
            );
        }
    };

    // Create new epoch key
    let new_routing_key = routing_key.with_next_epoch();
    let new_key_str = new_routing_key.to_key_string();

    // Create the new session
    match manager.get_or_create(&new_routing_key).await {
        Ok(_meta) => JsonRpcResponse::success(
            request.id,
            json!({
                "old_session_key": session_key_str,
                "new_session_key": new_key_str,
                "topic": topic,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create new session: {e}"),
        ),
    }
}
