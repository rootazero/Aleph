//! Database-backed session handlers using SessionManager.
//!
//! All `handle_*_db` functions operate against the SQLite-backed SessionManager
//! for production use.

use serde_json::{json, Value};
use crate::sync_primitives::Arc;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionManager, StoredMessage};

use super::store::{SessionInfo, HistoryMessage};

/// Handle sessions.list RPC request with database backend
pub async fn handle_list_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let agent_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("agent_id"))
        .and_then(|v| v.as_str());

    match manager.list_sessions(agent_id).await {
        Ok(sessions) => {
            let infos: Vec<SessionInfo> = sessions
                .into_iter()
                .map(|m| {
                    // Extract topic and status from metadata JSON
                    let (topic, status) = m.metadata_json
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        .map(|v| {
                            let topic = v.get("topic").and_then(|t| t.as_str()).map(|s| s.to_string());
                            let status = v.get("status").and_then(|t| t.as_str()).map(|s| s.to_string());
                            (topic, status)
                        })
                        .unwrap_or((None, None));

                    SessionInfo {
                        key: m.key,
                        agent_id: m.agent_id,
                        session_type: m.session_type,
                        message_count: m.message_count as u32,
                        created_at: chrono::DateTime::from_timestamp(m.created_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                        last_active_at: chrono::DateTime::from_timestamp(m.last_active_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                        topic,
                        status,
                    }
                })
                .collect();
            let count = infos.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "sessions": infos,
                    "count": count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list sessions: {}", e),
        ),
    }
}

/// Handle sessions.history RPC request with database backend
pub async fn handle_history_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
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

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    // Parse session key from string
    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format");
        }
    };

    match manager.get_history(&session_key, limit).await {
        Ok(messages) => {
            let history: Vec<HistoryMessage> = messages
                .into_iter()
                .map(|m| HistoryMessage {
                    role: m.role,
                    content: m.content,
                    timestamp: chrono::DateTime::from_timestamp(m.timestamp, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    metadata: m.metadata.map(|s| {
                        serde_json::from_str(&s).unwrap_or(Value::Null)
                    }),
                })
                .collect();
            let count = history.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key_str,
                    "messages": history,
                    "count": count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get history: {}", e),
        ),
    }
}

/// Handle sessions.reset RPC request with database backend
pub async fn handle_reset_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
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
                    format!("Failed to reset session: {}", e),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.delete RPC request with database backend
pub async fn handle_delete_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
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

            match manager.delete_session(&session_key).await {
                Ok(deleted) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "deleted": deleted,
                    }),
                ),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to delete session: {}", e),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle session.usage RPC request with database backend
pub async fn handle_usage_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
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

    // Get session metadata for timestamps
    match manager.list_sessions(None).await {
        Ok(sessions) => {
            let session_meta = sessions.iter().find(|s| s.key == session_key);

            // Get history for token estimation
            let messages = manager.get_history(&key, None).await.unwrap_or_default();
            let (input_tokens, output_tokens) = estimate_db_tokens(&messages);
            let total = input_tokens + output_tokens;

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key,
                    "tokens": total,
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "messages": messages.len(),
                    "created_at": session_meta.map(|s| {
                        chrono::DateTime::from_timestamp(s.created_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    "last_active_at": session_meta.map(|s| {
                        chrono::DateTime::from_timestamp(s.last_active_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to query sessions: {}", e),
        ),
    }
}

/// Handle session.create RPC request with database backend
///
/// Creates a new session and returns the session key and optional name.
pub async fn handle_create_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let name = request
        .params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Generate a unique session key based on timestamp
    let ts = chrono::Utc::now().timestamp_millis();
    let session_key_str = format!("session_{}", ts);
    let session_key = SessionKey::Main {
        agent_id: name.clone().unwrap_or_else(|| "main".to_string()),
        main_key: session_key_str.clone(),
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
            format!("Failed to create session: {}", e),
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
    manager: Arc<SessionManager>,
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
    if let Err(e) = manager.close_session(&legacy_key, topic.clone()).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to close session: {}", e),
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
    let new_legacy_key = SessionKey::from_new(&new_routing_key);

    // Create the new session
    match manager.get_or_create(&new_legacy_key).await {
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
            format!("Failed to create new session: {}", e),
        ),
    }
}

/// Handle session.compact RPC request with database backend
pub async fn handle_compact_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
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
    let before_msgs = manager
        .get_history(&key, None)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    match manager.compact_session(&key).await {
        Ok(deleted) => {
            let after_msgs = before_msgs.saturating_sub(deleted);
            let tokens_saved = deleted * 50; // rough estimate per message

            JsonRpcResponse::success(
                request.id,
                json!({
                    "message": if deleted > 0 {
                        format!("Compacted {} messages.", deleted)
                    } else {
                        "Session is already compact.".to_string()
                    },
                    "before_messages": before_msgs,
                    "after_messages": after_msgs,
                    "tokens_saved": tokens_saved,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Compact failed: {}", e),
        ),
    }
}

/// Handle sessions.set_topic RPC request with database backend
///
/// Params:
///   - session_key (required): session key string
///   - topic (required): new topic string (max 100 chars)
pub async fn handle_set_topic_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
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
        &topic[..topic.char_indices().nth(100).map(|(i, _)| i).unwrap_or(topic.len())]
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
            format!("Failed to set topic: {}", e),
        ),
    }
}

/// Estimate tokens from StoredMessage history
fn estimate_db_tokens(messages: &[StoredMessage]) -> (u64, u64) {
    let mut input = 0u64;
    let mut output = 0u64;
    for msg in messages {
        let tokens = (msg.content.len() as u64) / 3;
        match msg.role.as_str() {
            "user" => input += tokens,
            "assistant" => output += tokens,
            _ => input += tokens,
        }
    }
    (input, output)
}
