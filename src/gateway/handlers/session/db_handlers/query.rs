//! Query handlers for session database operations.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;

use super::types::{HistoryMessage, SessionInfo};

/// Handle sessions.list RPC request with database backend
pub async fn handle_list_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let agent_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("agent_id"))
        .and_then(|v| v.as_str());

    let filter = SessionFilter {
        agent_id: agent_id.map(|s| s.to_string()),
        ..Default::default()
    };
    match manager.list_sessions(filter).await {
        Ok(sessions) => {
            let infos: Vec<SessionInfo> = sessions
                .into_iter()
                // Filter out internal sessions (heartbeat tasks, cron tasks, ephemeral)
                // that should not appear in user-facing session lists
                .filter(|m| m.session_type != "task" && m.session_type != "ephemeral")
                .map(|m| {
                    let topic = m.derived_title.clone().or_else(|| {
                        m.topic.clone().or_else(|| {
                            m.identity_meta.as_ref().and_then(|im| {
                                im.custom
                                    .get("topic")
                                    .and_then(|v| v.as_str())
                                    .map(String::from)
                            })
                        })
                    });
                    let status = m.status.clone().or_else(|| {
                        m.identity_meta.as_ref().and_then(|im| {
                            im.custom
                                .get("status")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                        })
                    });
                    // Derive origin channel before the struct literal moves m's fields.
                    let channel = m.origin_channel();

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
                        state: m.state.map(|s| s.to_string()),
                        label: m.label,
                        input_tokens: m.input_tokens as u64,
                        output_tokens: m.output_tokens as u64,
                        model: m.model,
                        model_provider: m.model_provider,
                        parent_session_key: m.parent_session_key,
                        compaction_count: m.compaction_count as u64,
                        channel,
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
            format!("Failed to list sessions: {e}"),
        ),
    }
}

/// Handle sessions.history RPC request with database backend
pub async fn handle_history_db(
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

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    // Parse session key from string
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
                    metadata: m.metadata,
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
            format!("Failed to get history: {e}"),
        ),
    }
}

/// Handle session.usage RPC request with database backend
pub async fn handle_usage_db(
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

    // Get session metadata for usage stats
    match manager.list_sessions(SessionFilter::default()).await {
        Ok(sessions) => {
            let session_meta = sessions.iter().find(|s| s.key == session_key);

            let (input_tokens, output_tokens, total, message_count, created_at, last_active_at) =
                session_meta.map_or((0, 0, 0, 0, None, None), |s| {
                    (
                        s.input_tokens as u64,
                        s.output_tokens as u64,
                        s.total_tokens as u64,
                        s.message_count as u64,
                        chrono::DateTime::from_timestamp(s.created_at, 0).map(|dt| dt.to_rfc3339()),
                        chrono::DateTime::from_timestamp(s.last_active_at, 0)
                            .map(|dt| dt.to_rfc3339()),
                    )
                });

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key,
                    "tokens": total,
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "messages": message_count,
                    "created_at": created_at,
                    "last_active_at": last_active_at,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to query sessions: {e}"),
        ),
    }
}

/// Handle sessions.preview RPC request with database backend
pub async fn handle_preview_db(
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

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map_or(10, |n| n as usize);

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

    match manager.get_session_preview(&session_key, limit).await {
        Ok(preview) => {
            let messages: Vec<Value> = preview
                .messages
                .into_iter()
                .map(|m| {
                    json!({
                        "role": m.role,
                        "content": m.content,
                        "timestamp": chrono::DateTime::from_timestamp(m.timestamp, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                        "metadata": m.metadata,
                    })
                })
                .collect();

            let meta_json = preview.meta.map(|m| {
                json!({
                    "key": m.key,
                    "agent_id": m.agent_id,
                    "session_type": m.session_type,
                    "message_count": m.message_count,
                    "total_tokens": m.total_tokens,
                    "state": m.state.map(|s| s.to_string()),
                    "label": m.label,
                    "input_tokens": m.input_tokens,
                    "output_tokens": m.output_tokens,
                    "model": m.model,
                    "model_provider": m.model_provider,
                    "parent_session_key": m.parent_session_key,
                    "compaction_count": m.compaction_count,
                    "derived_title": m.derived_title,
                    "last_message_preview": m.last_message_preview,
                    "runtime_ms": m.runtime_ms,
                    "estimated_cost_usd": m.estimated_cost_usd,
                    "checkpoints": m.checkpoints.into_iter().map(|c| json!({
                        "checkpoint_id": c.checkpoint_id,
                        "created_at": chrono::DateTime::from_timestamp(c.created_at, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                        "message_count": c.message_count,
                        "retained_message_count": c.retained_message_count,
                    })).collect::<Vec<_>>(),
                    "created_at": chrono::DateTime::from_timestamp(m.created_at, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    "last_active_at": chrono::DateTime::from_timestamp(m.last_active_at, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                })
            });

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": session_key_str,
                    "meta": meta_json,
                    "messages": messages,
                    "message_count": messages.len(),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get session preview: {e}"),
        ),
    }
}
