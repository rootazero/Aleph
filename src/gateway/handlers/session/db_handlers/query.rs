//! Query handlers for session database operations.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

use super::types::HistoryMessage;
use crate::session::events::SessionEventRecord;
use aleph_protocol::SessionListRow;

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
        owner_visible_to: visibility::visible_owner_filter(),
        ..Default::default()
    };
    // One indexed cross-session query for the whole page, not one per row.
    //
    // `None` — no event store published, or the read failed — leaves every
    // row's `last_run` at `None`, which reads as "we did not find out". The
    // arm that must never exist is one that renders a session as clean because
    // nobody asked: the interrupted rows are exactly the ones this column is
    // for (criterion #8).
    let run_markers: Option<std::collections::HashMap<String, Vec<SessionEventRecord>>> =
        match crate::session::store::global_session_event_store() {
            Some(events) => match events.load_run_markers().await {
                Ok(rows) => Some(
                    rows.into_iter()
                        .map(|(session, markers)| (session.to_key_string(), markers))
                        .collect(),
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "sessions.list could not read run markers");
                    None
                }
            },
            None => None,
        };

    match manager.list_sessions(filter).await {
        Ok(sessions) => {
            let infos: Vec<SessionListRow> = sessions
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
                    // Every per-session knob, decoded by the one function
                    // `chat.history` also uses. These used to be three inline
                    // `im.custom.get(...)` reads here and zero anywhere else,
                    // which is how `think_level` — persisted since the day it
                    // was added — reached no client surface at all: the shape is
                    // trivial, so it got copied per knob instead of shared, and
                    // the copy nobody wrote is invisible.
                    let snapshot = crate::gateway::session_snapshot::snapshot_from_metadata(&m);
                    let project_root = snapshot.project_root;
                    let exec_tier = snapshot.exec_tier;
                    let mode = snapshot.mode;
                    let think_level = snapshot.think_level;
                    let memory_mode = snapshot.memory_mode;
                    let model_pin = snapshot.model_pin;

                    // A session absent from the marker map has no run markers
                    // at all — `never_ran`, which is a different answer from
                    // "its last run finished". Derived through the same
                    // reducer the attach face uses, so the two faces cannot
                    // disagree about the word.
                    let last_run = run_markers.as_ref().map(|by_session| {
                        crate::gateway::session_snapshot::last_run_from_markers(
                            by_session.get(&m.key).map_or(&[][..], Vec::as_slice),
                        )
                    });

                    SessionListRow {
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
                        exec_tier,
                        mode,
                        think_level,
                        memory_mode,
                        model_pin,
                        last_run,
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

    let meta = match manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        return visibility::not_found_response(request.id); // same error as missing (GC 4)
    }

    match manager.get_history(&session_key, limit).await {
        Ok(messages) => {
            let history: Vec<HistoryMessage> = messages
                .into_iter()
                .map(|m| HistoryMessage {
                    // Timestamp first: `role`/`content` move out of `m`, and
                    // the accessor needs `m` whole.
                    timestamp: m.rfc3339(),
                    role: m.role,
                    content: m.content,
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

/// USD cost for a session's token usage, and how much to trust it.
///
/// `accumulated_usd` — `SessionMetadata::estimated_cost_usd` — is authoritative
/// and is returned as-is when present: the projector adds each run's own priced
/// figure, computed from the **full** `TokenBreakdown` at the model that run
/// actually used. Recomputing from the session row instead throws all of that
/// away, because the row keeps only `input_tokens` / `output_tokens`: cached
/// prompt bytes (~0.1x) and cache writes (1.25x) get re-priced as ordinary
/// input, and a multi-model session collapses onto whichever model happened to
/// be stamped last.
///
/// The recompute survives only as a fallback for rows that predate the
/// accumulation (or a run that could not be priced), and it says so — a
/// cache-blind derivation must never come back labelled `"complete"`, because
/// that is exactly the word clients render without a `≈`.
///
/// Pure over its inputs so it stays unit-testable without a SessionStore, and
/// so all pricing lives in core (R4 — the shells own no price table).
fn session_usage_cost(
    accumulated_usd: f64,
    provider: Option<&str>,
    model: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
) -> (Option<f64>, &'static str) {
    if accumulated_usd > 0.0 {
        return (Some(accumulated_usd), "complete");
    }
    let (Some(provider), Some(model)) = (provider, model) else {
        return (None, "unknown");
    };
    let breakdown = crate::orchestrator::dispatch::TokenBreakdown {
        input: u32::try_from(input_tokens).unwrap_or(u32::MAX),
        output: u32::try_from(output_tokens).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let est = crate::pricing::estimate(provider, model, &breakdown);
    match est.status {
        crate::pricing::CostStatus::Unknown => (None, "unknown"),
        // Priced, but from a breakdown with no cache split — an estimate, and
        // labelled as one so no client renders it as the exact figure.
        crate::pricing::CostStatus::Complete => (Some(est.usd), CACHE_BLIND_ESTIMATE),
        crate::pricing::CostStatus::PartialMissingPrice => (Some(est.usd), "partial_missing_price"),
    }
}

/// `cost_status` for a figure derived from `input`/`output` alone. Any status
/// other than `"complete"` is rendered as an approximation by clients, which is
/// the whole point — see [`session_usage_cost`].
const CACHE_BLIND_ESTIMATE: &str = "estimated_no_cache_split";

/// Handle session.usage RPC request with database backend
pub async fn handle_usage_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let session_key_str = match request
        .params
        .as_ref()
        .and_then(|p| p.get("session_key"))
        .and_then(|v| v.as_str())
    {
        Some(k) => k.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let session_key = match SessionKey::from_key_string(&session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    // Fetch metadata directly by key rather than scanning every session
    // (the old implementation called `list_sessions(SessionFilter::default())`
    // and linear-searched) — this is also the enforcement point: a session
    // that exists but belongs to someone else must read exactly like one
    // that doesn't exist (GC 4).
    let session_meta = match manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&session_meta) {
        return visibility::not_found_response(request.id); // same error as missing (GC 4)
    }

    let input_tokens = session_meta.input_tokens as u64;
    let output_tokens = session_meta.output_tokens as u64;
    let total = session_meta.total_tokens as u64;
    let message_count = session_meta.message_count as u64;
    let created_at =
        chrono::DateTime::from_timestamp(session_meta.created_at, 0).map(|dt| dt.to_rfc3339());
    let last_active_at =
        chrono::DateTime::from_timestamp(session_meta.last_active_at, 0).map(|dt| dt.to_rfc3339());

    let (cost_usd, cost_status) = session_usage_cost(
        session_meta.estimated_cost_usd,
        session_meta.model_provider.as_deref(),
        session_meta.model.as_deref(),
        input_tokens,
        output_tokens,
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "session_key": session_key_str,
            "tokens": total,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "messages": message_count,
            "created_at": created_at,
            "last_active_at": last_active_at,
            "cost_usd": cost_usd,
            "cost_status": cost_status,
        }),
    )
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

    let meta = match manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        return visibility::not_found_response(request.id); // same error as missing (GC 4)
    }

    match manager.get_session_preview(&session_key, limit).await {
        Ok(preview) => {
            let messages: Vec<Value> = preview
                .messages
                .into_iter()
                .map(|m| {
                    json!({
                        "role": m.role,
                        "content": m.content,
                        "timestamp": m.rfc3339(),
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
    use super::{
        clean_derived_title, handle_list_db, resolve_display_title, session_usage_cost,
        CACHE_BLIND_ESTIMATE,
    };
    use crate::gateway::protocol::JsonRpcRequest;
    use crate::gateway::router::SessionKey;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig, SessionPatch};
    use crate::gateway::session_store::SessionStore;
    use crate::sync_primitives::Arc;
    use aleph_protocol::SessionListRow;
    use serde_json::json;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    /// P1 visibility chokepoint — pinned per task-6-brief.md Step 1. Same
    /// discipline as `owner_scope_tests` in `session_store/mod.rs`: dispatch
    /// through the real handler with `CALLER_USER` scoped around the call,
    /// exactly as `process_request` scopes it in production.
    mod visibility_guards {
        use super::super::{handle_history_db, handle_preview_db, handle_usage_db};
        use super::*;
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::gateway::protocol::RESOURCE_NOT_FOUND;
        use crate::scope::{with_scope, ScopeAttribution};

        fn store(temp: &tempfile::TempDir) -> Arc<dyn SessionStore> {
            Arc::new(
                SessionManager::new(SessionManagerConfig {
                    db_path: temp.path().join("visibility.db"),
                    ..Default::default()
                })
                .unwrap(),
            )
        }

        fn list_request() -> JsonRpcRequest {
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.list".into(),
                params: None,
                id: Some(json!(1)),
            }
        }

        fn keyed_request(method: &str, session_key: &str) -> JsonRpcRequest {
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: method.into(),
                params: Some(json!({ "session_key": session_key })),
                id: Some(json!(1)),
            }
        }

        async fn seed(store: &Arc<dyn SessionStore>) -> (SessionKey, SessionKey, SessionKey) {
            let alice_1 = SessionKey::from_key_string("agent:alicevis1:main").unwrap();
            let alice_2 = SessionKey::from_key_string("agent:alicevis2:main").unwrap();
            let bob_1 = SessionKey::from_key_string("agent:bobvis1:main").unwrap();
            let legacy = SessionKey::from_key_string("agent:legacyvis1:main").unwrap();

            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                store.get_or_create(&alice_1),
            )
            .await
            .unwrap();
            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                store.get_or_create(&alice_2),
            )
            .await
            .unwrap();
            with_scope(
                Some(ScopeAttribution::personal("u-bob")),
                store.get_or_create(&bob_1),
            )
            .await
            .unwrap();
            store.get_or_create(&legacy).await.unwrap();

            (alice_1, bob_1, legacy)
        }

        /// spec §9-1 shape: alice creates 2, bob creates 1, one legacy row.
        /// Dispatching `sessions.list` under each caller's scope must return
        /// exactly that caller's own sessions.
        #[tokio::test]
        async fn sessions_list_is_scoped_to_the_caller() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            seed(&store).await;

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_list_db(list_request(), store.clone()),
                )
                .await;
            let bob_sessions = as_bob.result.unwrap()["sessions"].as_array().unwrap().len();
            assert_eq!(bob_sessions, 1, "bob sees only his own session");

            let as_owner = CALLER_USER
                .scope(
                    Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
                    handle_list_db(list_request(), store.clone()),
                )
                .await;
            let owner_keys: Vec<String> = as_owner.result.unwrap()["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["key"].as_str().unwrap().to_string())
                .collect();
            assert!(
                !owner_keys.iter().any(|k| k.contains("bobvis1")),
                "owner must not see bob's session"
            );
            assert!(
                owner_keys.iter().any(|k| k.contains("legacyvis1")),
                "owner must see the legacy (unscoped) row — legacy = owner"
            );

            let unrestricted = handle_list_db(list_request(), store.clone()).await;
            let all = unrestricted.result.unwrap()["sessions"]
                .as_array()
                .unwrap()
                .len();
            assert_eq!(all, 4, "no CALLER_USER scope = every owner");
        }

        #[tokio::test]
        async fn sessions_history_denies_cross_user_as_not_found() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let (alice_key, _bob_key, _legacy) = seed(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_history_db(
                        keyed_request("sessions.history", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );

            // Byte-identical to a genuinely nonexistent key (no existence oracle).
            let never_existed = "agent:nosuchsession999:main";
            let as_bob_missing = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_history_db(
                        keyed_request("sessions.history", never_existed),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                serde_json::to_string(&as_bob.error).unwrap(),
                serde_json::to_string(&as_bob_missing.error).unwrap(),
            );

            // alice herself, and an unrestricted (internal/no-scope) caller,
            // may still read it. Note OWNER_USER_ID is NOT special here — a
            // stamped session belongs exclusively to its stamped owner, not
            // to the org-era owner id; only a legacy (never-stamped) row
            // reads as OWNER_USER_ID's.
            let as_alice = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_history_db(
                        keyed_request("sessions.history", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert!(as_alice.error.is_none());

            let unrestricted = handle_history_db(
                keyed_request("sessions.history", &alice_key_str),
                store.clone(),
            )
            .await;
            assert!(unrestricted.error.is_none());
        }

        #[tokio::test]
        async fn sessions_preview_denies_cross_user_as_not_found() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let (alice_key, _bob_key, _legacy) = seed(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_preview_db(
                        keyed_request("sessions.preview", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }

        #[tokio::test]
        async fn session_usage_denies_cross_user_as_not_found() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let (alice_key, _bob_key, _legacy) = seed(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_usage_db(
                        keyed_request("session.usage", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }
    }

    /// The run loop resolves a stored `exec_tier` on every turn, so the Panel
    /// must be able to read it back after a reload. Without this projection the
    /// tier pill reports "follow global" for a session the server is still
    /// gating at the pinned tier — the UI under-reporting a live security
    /// control, in the permissive direction.
    #[tokio::test]
    async fn list_projects_the_session_exec_tier_override() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("tier_list.db"),
            ..Default::default()
        })
        .unwrap();

        let pinned = SessionKey::from_key_string("agent:tierpinned:main").unwrap();
        let plain = SessionKey::from_key_string("agent:tierplain:main").unwrap();
        manager.get_or_create(&pinned).await.unwrap();
        manager.get_or_create(&plain).await.unwrap();
        manager
            .patch_session(
                &pinned,
                &SessionPatch {
                    metadata: Some(json!({ "exec_tier": "full" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let response = handle_list_db(
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.list".into(),
                params: None,
                id: Some(json!(1)),
            },
            store,
        )
        .await;
        let result = response.result.expect("result");
        let sessions: Vec<SessionListRow> =
            serde_json::from_value(result["sessions"].clone()).expect("sessions");

        let tier_of = |agent: &str| -> Option<String> {
            sessions
                .iter()
                .find(|s| s.agent_id == agent)
                .expect("session listed")
                .exec_tier
                .clone()
        };
        assert_eq!(tier_of("tierpinned"), s("full"));
        assert_eq!(
            tier_of("tierplain"),
            None,
            "a session with no override must follow the global tier"
        );
    }

    /// What `sessions.list` emits is what [`SessionListRow`] declares — the
    /// server constructs the shared type, so a client parsing it cannot be
    /// reading a different shape.
    ///
    /// `topic` and `label` are asserted by name because a client used to look
    /// for a key called `name`, found nothing, and fell back to the session key
    /// — every conversation in that picker was titled by its key while the two
    /// real titles rode the wire unread. Parsing alone would not have caught
    /// it: a partial reader always parses (criterion #10).
    #[tokio::test]
    async fn a_listed_session_is_the_protocol_row_with_its_title_on_it() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("row_shape.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:rowshape:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        manager
            .patch_session(
                &key,
                &SessionPatch {
                    label: Some("r2".to_string()),
                    metadata: Some(json!({ "topic": "crash recovery" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let response = handle_list_db(
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.list".into(),
                params: None,
                id: Some(json!(1)),
            },
            store,
        )
        .await;
        let result = response.result.expect("result");
        let raw = result["sessions"][0].clone();
        assert_eq!(
            raw["topic"], "crash recovery",
            "the display title must reach the wire under the key clients read"
        );
        assert_eq!(raw["label"], "r2");

        let rows: Vec<SessionListRow> =
            serde_json::from_value(result["sessions"].clone()).expect("the shared row type");
        assert_eq!(rows[0].topic.as_deref(), Some("crash recovery"));
        assert_eq!(rows[0].label.as_deref(), Some("r2"));
        assert_eq!(rows[0].key, "agent:rowshape:main");
    }

    #[test]
    fn llm_topic_wins_over_derived_first_message() {
        // The core fix: once the async LLM topic lands, it takes precedence over
        // the raw first-message title so the sidebar shows the concise name.
        assert_eq!(
            resolve_display_title(
                s("冒烟工作目录"),
                None,
                s("冒烟测试：你的当前工作目录是什么？")
            ),
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
            resolve_display_title(
                None,
                s("创建MOA预设"),
                s("<system-reminder>\nWorking directory")
            ),
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

    #[test]
    fn session_usage_cost_prefers_the_accumulated_figure() {
        // The projector already added each run's cache-aware price. That figure
        // wins outright — it is the only one that ever saw `cache_read` /
        // `cache_creation`, and it is the only one entitled to "complete".
        let (usd, status) = session_usage_cost(
            4.25,
            Some("anthropic"),
            Some("claude-sonnet-4-6"),
            1_000_000,
            1_000_000,
        );
        assert_eq!(status, "complete");
        assert_eq!(usd, Some(4.25));
    }

    #[test]
    fn a_cache_blind_recompute_never_claims_to_be_complete() {
        // No accumulated cost (a pre-accumulation row): the fallback may still
        // price the tokens, but it priced cached bytes as full-rate input, so
        // it must not come back wearing the word clients render without a `≈`.
        let (usd, status) = session_usage_cost(
            0.0,
            Some("anthropic"),
            Some("claude-sonnet-4-6"),
            1_000_000,
            1_000_000,
        );
        assert!(usd.unwrap() > 0.0);
        assert_ne!(status, "complete");
        assert_eq!(status, CACHE_BLIND_ESTIMATE);
    }

    #[test]
    fn session_usage_cost_unknown_without_price() {
        assert_eq!(
            session_usage_cost(0.0, None, None, 100, 100),
            (None, "unknown")
        );
        assert_eq!(
            session_usage_cost(0.0, Some("anthropic"), Some("no-such-model"), 100, 100).1,
            "unknown"
        );
    }
}
