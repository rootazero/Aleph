//! Checkpoint handlers for session compaction.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;

/// Handle sessions.compaction.list RPC request
pub async fn handle_list_checkpoints_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let session_key = match request
        .params
        .as_ref()
        .and_then(|p| p.get("session_key"))
        .and_then(|v| v.as_str())
    {
        Some(k) => k.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let key = match SessionKey::from_key_string(&session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    match manager.list_checkpoints(&key).await {
        Ok(checkpoints) => {
            let items: Vec<Value> = checkpoints
                .into_iter()
                .map(|c| {
                    json!({
                        "checkpoint_id": c.checkpoint_id,
                        "created_at": chrono::DateTime::from_timestamp(c.created_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                        "message_count": c.message_count,
                        "retained_message_count": c.retained_message_count,
                    })
                })
                .collect();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key,
                    "checkpoints": items,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list checkpoints: {e}"),
        ),
    }
}

/// Handle sessions.compaction.restore RPC request
pub async fn handle_restore_checkpoint_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let checkpoint_id = match params.get("checkpoint_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing checkpoint_id");
        }
    };

    let key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    match manager.restore_checkpoint(&key, checkpoint_id).await {
        Ok(meta) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "checkpoint_id": checkpoint_id,
                "message_count": meta.message_count,
                "updated": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to restore checkpoint: {e}"),
        ),
    }
}

/// Handle sessions.compaction.branch RPC request
pub async fn handle_branch_checkpoint_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let checkpoint_id = match params.get("checkpoint_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing checkpoint_id");
        }
    };

    let new_session_key_str = match params.get("new_session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing new_session_key");
        }
    };

    let key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let new_key = match SessionKey::from_key_string(new_session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid new_session_key format",
            );
        }
    };

    match manager
        .branch_from_checkpoint(&key, checkpoint_id, &new_key)
        .await
    {
        Ok(meta) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "checkpoint_id": checkpoint_id,
                "new_session_key": new_session_key_str,
                "message_count": meta.message_count,
                "created": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to branch checkpoint: {e}"),
        ),
    }
}
