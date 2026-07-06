//! Query handlers for session database operations.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;

use super::types::{HistoryMessage, SessionInfo};

/// A session's `derived_title` is computed from the first user message's raw
/// content. Sessions seeded before the raw-input persistence fix leaked the
/// ephemeral per-turn `<system-reminder>` working-directory block into that
/// content, so their stored title is a truncated reminder fragment rather than
/// the user's text. Treat such a leaked title as absent so a real title is
/// shown instead.
fn clean_derived_title(title: Option<String>) -> Option<String> {
    title.filter(|t| !t.trim_start().starts_with("<system-reminder>"))
}

/// Resolve a session's sidebar title. The LLM-generated `topic` (auto-named on
/// the first turn, stored top-level or under `identity_meta.custom.topic`) is
/// authoritative; the first-message-derived title is only a fallback — shown as
/// an instant placeholder until the async topic lands, or when topic generation
/// produced nothing. A leaked `<system-reminder>` derived title is scrubbed so
/// it never surfaces even as the fallback.
fn resolve_display_title(
    topic: Option<String>,
    identity_topic: Option<String>,
    derived_title: Option<String>,
) -> Option<String> {
    topic
        .or(identity_topic)
        .or_else(|| clean_derived_title(derived_title))
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
                    let identity_topic = m.identity_meta.as_ref().and_then(|im| {
                        im.custom
                            .get("topic")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    });
                    let topic = resolve_display_title(
                        m.topic.clone(),
                        identity_topic,
                        m.derived_title.clone(),
                    );
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
                    let updated_at = m.last_active_at;
                    // User-chosen project working directory, persisted via
                    // `sessions.set_project_root` into identity_meta.custom.
                    let project_root = m.identity_meta.as_ref().and_then(|im| {
                        im.custom
                            .get("project_root")
                            .and_then(|v| v.as_str())
                            .map(String::from)
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
                        channel,
                        updated_at,
                        project_root,
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

#[cfg(test)]
mod tests {
    use super::{clean_derived_title, resolve_display_title};

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn llm_topic_wins_over_derived_first_message() {
        // The core fix: once the async LLM topic lands, it takes precedence over
        // the raw first-message title so the sidebar shows the concise name.
        assert_eq!(
            resolve_display_title(s("冒烟工作目录"), None, s("冒烟测试：你的当前工作目录是什么？")),
            s("冒烟工作目录")
        );
    }

    #[test]
    fn identity_topic_wins_over_derived_first_message() {
        // File backend stores the auto-topic under identity_meta.custom.topic.
        assert_eq!(
            resolve_display_title(None, s("MOA状态查询"), s("请查询一下 MOA 的当前状态好吗")),
            s("MOA状态查询")
        );
    }

    #[test]
    fn derived_title_is_the_placeholder_until_topic_lands() {
        // No topic yet (generation still async / not run) → show the raw message.
        assert_eq!(
            resolve_display_title(None, None, s("你当前的工作目录是什么？")),
            s("你当前的工作目录是什么？")
        );
    }

    #[test]
    fn leaked_derived_title_scrubbed_when_no_topic() {
        assert_eq!(
            resolve_display_title(
                None,
                None,
                s("<system-reminder>\nWorking directory: `/Users/x/.aleph`")
            ),
            None
        );
    }

    #[test]
    fn topic_shown_even_when_derived_title_is_leaked() {
        // Old polluted sessions that already have an LLM topic still render it.
        assert_eq!(
            resolve_display_title(None, s("创建MOA预设"), s("<system-reminder>\nWorking directory")),
            s("创建MOA预设")
        );
    }

    #[test]
    fn all_absent_resolves_to_none() {
        assert_eq!(resolve_display_title(None, None, None), None);
    }

    #[test]
    fn leaked_system_reminder_title_is_dropped() {
        // The pre-fix leak: title truncated mid-reminder, no user text survives.
        let leaked =
            Some("<system-reminder>\nWorking directory: `/Users/x/.aleph/workspaces".to_string());
        assert_eq!(clean_derived_title(leaked), None);
    }

    #[test]
    fn leaked_title_with_leading_whitespace_is_dropped() {
        let leaked = Some("  \n<system-reminder>\nWorking directory: `/tmp`".to_string());
        assert_eq!(clean_derived_title(leaked), None);
    }

    #[test]
    fn clean_user_title_passes_through() {
        let ok = Some("帮我做一个发布会 PPT".to_string());
        assert_eq!(clean_derived_title(ok.clone()), ok);
    }

    #[test]
    fn none_stays_none() {
        assert_eq!(clean_derived_title(None), None);
    }
}
