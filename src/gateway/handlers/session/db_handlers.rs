//! Session handlers operating against the SessionStore trait.
//!
//! All `handle_*_db` functions operate against any `SessionStore` implementation
//! (SQLite or file backend) for production use.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;

/// Session information returned by list handlers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    /// Session key string
    pub key: String,
    /// Agent ID
    pub agent_id: String,
    /// Session type (main, peer, task, ephemeral)
    pub session_type: String,
    /// Message count in session
    pub message_count: u32,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
    /// Last activity timestamp (ISO 8601)
    pub last_active_at: String,
    /// Session topic (extracted from metadata JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Session status (e.g. "closed")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Current lifecycle state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// User-facing label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Input tokens consumed
    pub input_tokens: u64,
    /// Output tokens consumed
    pub output_tokens: u64,
    /// Model used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Model provider used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    /// Parent session key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    /// Number of compactions performed
    pub compaction_count: u64,
}

/// Session history message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

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
                    let topic = m.topic.clone().or_else(|| {
                        m.identity_meta.as_ref().and_then(|im| {
                            im.custom.get("topic").and_then(|v| v.as_str()).map(String::from)
                        })
                    });
                    let status = m.status.clone().or_else(|| {
                        m.identity_meta.as_ref().and_then(|im| {
                            im.custom.get("status").and_then(|v| v.as_str()).map(String::from)
                        })
                    });

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
            format!("Failed to get history: {}", e),
        ),
    }
}

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
                    format!("Failed to reset session: {}", e),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.delete RPC request with database backend.
///
/// Before dropping the transcript we capture its tail into `raw_memories`
/// as a `SessionEnd` raw so the CompressionService / ProfileSynthesizer
/// can mine durable knowledge from the dying session. Without this hook
/// `USER.md` never updates and per-session digests are silently lost.
pub async fn handle_delete_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    handle_delete_db_inner(request, manager, None).await
}

/// Variant accepting an explicit raw-memory writer for the SessionEnd capture.
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
                Ok(result) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "deleted": result.deleted,
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
        label: params.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()),
        status: params.get("status").and_then(|v| v.as_str()).map(|s| s.to_string()),
        model: params.get("model").and_then(|v| v.as_str()).map(|s| s.to_string()),
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
            format!("Failed to patch session: {}", e),
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
        .map(|n| n as usize)
        .unwrap_or(10);

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
            format!("Failed to get session preview: {}", e),
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
                session_meta
                    .map(|s| {
                        (
                            s.input_tokens as u64,
                            s.output_tokens as u64,
                            s.total_tokens as u64,
                            s.message_count as u64,
                            chrono::DateTime::from_timestamp(s.created_at, 0)
                                .map(|dt| dt.to_rfc3339()),
                            chrono::DateTime::from_timestamp(s.last_active_at, 0)
                                .map(|dt| dt.to_rfc3339()),
                        )
                    })
                    .unwrap_or((0, 0, 0, 0, None, None));

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
            format!("Failed to query sessions: {}", e),
        ),
    }
}

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
    let session_key_str = format!("session_{}", ts);
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
            format!("Failed to create new session: {}", e),
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
    let before_msgs = manager
        .get_history(&key, None)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    match manager.compact(&key, crate::gateway::session_store::types::CompactStrategy::KeepLastN { n: 50 }).await {
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
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Compact failed: {}", e))
        }
    }
}

/// Handle sessions.set_topic RPC request with database backend
///
/// Params:
///   - session_key (required): session key string
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
            .map(|(i, _)| i)
            .unwrap_or(topic.len())]
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
            format!("Failed to list checkpoints: {}", e),
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
            format!("Failed to restore checkpoint: {}", e),
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

    match manager.branch_from_checkpoint(&key, checkpoint_id, &new_key).await {
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
            format!("Failed to branch checkpoint: {}", e),
        ),
    }
}


