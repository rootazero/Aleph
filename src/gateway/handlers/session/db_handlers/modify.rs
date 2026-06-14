//! Session modification handlers (reset, delete, patch, compact, `set_topic`).

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

/// Fire the `SessionEnd` extension hook (observers) for a deleted session.
///
/// Best-effort: a missing extension manager, an empty hook set, or a hook
/// failure is ignored — session deletion must never depend on hook
/// availability. Lives here (not in `src/harness/`) so the dumb loop stays
/// free of lifecycle logic (R10).
async fn fire_session_end_hook(session_key: &crate::gateway::router::SessionKey) {
    let Ok(manager) = crate::gateway::handlers::plugins::get_extension_manager() else {
        return;
    };
    let executor = manager.hook_executor_snapshot().await;
    if executor.hook_count() == 0 {
        return;
    }
    let ctx = crate::extension::hooks::HookContext::new(session_key.to_key_string())
        .with_env("AGENT_ID", session_key.agent_id());
    executor
        .execute_observers(crate::extension::HookEvent::SessionEnd, &ctx)
        .await;
}

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;

/// Handle sessions.reset RPC request with database backend
pub async fn handle_reset_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let session_key_str = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key_str {
        Some(key_str) => {
            let session_key = match SessionKey::from_key_string(key_str) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid session_key format",
                    );
                }
            };

            match manager.reset_session(&session_key).await {
                Ok(reset) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "reset": reset,
                    }),
                ),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to reset session: {e}"),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.delete RPC request with database backend.
///
/// Before dropping the transcript we capture its tail into `raw_memories`
/// as a `SessionEnd` raw so the `CompressionService` / `ProfileSynthesizer`
/// can mine durable knowledge from the dying session. Without this hook
/// `USER.md` never updates and per-session digests are silently lost.
pub async fn handle_delete_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    handle_delete_db_inner(request, manager, None).await
}

/// Variant accepting an explicit raw-memory writer for the `SessionEnd` capture.
/// The default `handle_delete_db` keeps the writer optional for backwards
/// compatibility with the macro-generated registration shape.
pub async fn handle_delete_db_with_capture(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
    writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
) -> JsonRpcResponse {
    handle_delete_db_inner(request, manager, Some(writer)).await
}

async fn handle_delete_db_inner(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
    writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
) -> JsonRpcResponse {
    let session_key_str = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key_str {
        Some(key_str) => {
            let session_key = match SessionKey::from_key_string(key_str) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid session_key format",
                    );
                }
            };

            // Capture session tail BEFORE deletion so SessionEnd raw fires.
            if let Some(ref w) = writer {
                if let Ok(history) = manager.get_history(&session_key, Some(64)).await {
                    let tail = history
                        .iter()
                        .map(|m| format!("[{}] {}", m.role, m.content))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !tail.is_empty() {
                        crate::gateway::session_manager::ops::emit_session_end_raw(
                            Arc::clone(w),
                            session_key.agent_id().to_string(),
                            key_str.to_string(),
                            tail,
                            crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                        );
                    }
                }
            }

            match manager.delete_session(&session_key).await {
                Ok(result) => {
                    // SessionEnd — the session has been removed; extension
                    // observers witness the teardown.
                    fire_session_end_hook(&session_key).await;
                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "session_key": key_str,
                            "deleted": result.deleted,
                        }),
                    )
                }
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to delete session: {e}"),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.patch RPC request with database backend
pub async fn handle_patch_db(
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

    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let patch = crate::gateway::session_manager::SessionPatch {
        label: params
            .get("label")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        status: params
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model: params
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model_provider: params
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        metadata: params.get("metadata").cloned(),
    };

    match manager.patch_session(&session_key, &patch).await {
        Ok(updated) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "updated": updated,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to patch session: {e}"),
        ),
    }
}

/// Handle session.compact RPC request with database backend
pub async fn handle_compact_db(
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

    // Get message count before compact
    let before_msgs = manager.get_history(&key, None).await.map_or(0, |m| m.len());

    match manager
        .compact(
            &key,
            crate::gateway::session_store::types::CompactStrategy::KeepLastN { n: 50 },
        )
        .await
    {
        Ok(result) => {
            let after_msgs = before_msgs.saturating_sub(result.deleted);
            let tokens_saved = result.deleted * 50; // rough estimate per message

            JsonRpcResponse::success(
                request.id,
                json!({
                    "message": if result.deleted > 0 {
                        format!("Compacted {} messages.", result.deleted)
                    } else {
                        "Session is already compact.".to_string()
                    },
                    "before_messages": before_msgs,
                    "after_messages": after_msgs,
                    "tokens_saved": tokens_saved,
                }),
            )
        }
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Compact failed: {e}"))
        }
    }
}

/// Handle session.truncate RPC request with database backend.
///
/// Removes messages from the tail of a session, keeping only the first
/// `keep_count` messages by chronological order. Used by the TUI `/undo`
/// command to drop the most recent user+assistant turn pair.
pub async fn handle_truncate_db(
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
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let keep_count = match params.get("keep_count").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing keep_count"),
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

    match manager.truncate_messages(&key, keep_count).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "messages_removed": result.messages_removed,
                "tokens_removed_estimate": result.tokens_removed_estimate,
            }),
        ),
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Truncate failed: {e}"))
        }
    }
}

/// Handle `sessions.set_topic` RPC request with database backend
///
/// Params:
///   - `session_key` (required): session key string
///   - topic (required): new topic string (max 100 chars)
pub async fn handle_set_topic_db(
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

    let topic = match params.get("topic").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing topic");
        }
    };

    // Validate topic length (P7: boundary validation)
    let topic = if topic.len() > 100 {
        &topic[..topic
            .char_indices()
            .nth(100)
            .map_or(topic.len(), |(i, _)| i)]
    } else {
        topic
    };

    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    match manager.set_topic(&session_key, topic).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "updated": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set topic: {e}"),
        ),
    }
}
