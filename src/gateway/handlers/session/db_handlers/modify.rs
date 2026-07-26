//! Session modification handlers (reset, delete, patch, compact, `set_topic`).

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

/// Drop every artifact a deleted session produced.
///
/// Best-effort, exactly like [`fire_session_end_hook`]: the transcript is
/// already gone, so failing the RPC here would report a deletion that in fact
/// happened. The handler only says *which* session died — where the bytes live
/// and how they are reclaimed stays inside `crate::artifacts` (R4).
async fn purge_session_artifacts(session_key: &str) {
    let root = match crate::artifacts::ArtifactStore::default_root() {
        Ok(root) => root,
        Err(e) => {
            tracing::warn!(error = %e, "cannot resolve the artifact root; artifacts not purged");
            return;
        }
    };
    if let Err(e) = crate::artifacts::ArtifactStore::new(root)
        .purge_session(session_key)
        .await
    {
        tracing::warn!(error = %e, session_key, "failed to purge session artifacts");
    }
}

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

            // Retire the SSOT event log before the `messages` projection —
            // same ordering rationale as `chat.clear`: resetting only the
            // projection leaves the model replaying the cleared conversation.
            let retired = match crate::session::store::retire_live_events(&session_key, 1).await {
                Ok(n) => n,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to reset session event log: {e}"),
                    );
                }
            };

            match manager.reset_session(&session_key).await {
                Ok(reset) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "reset": reset || retired > 0,
                        "events_retired": retired,
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

            // Retire the SSOT event log before the projection — same ordering
            // as `chat.clear`. Deleting the `messages` rows alone would leave
            // the conversation alive in `session_events`: still replayed by the
            // model, still searchable, and re-materialised into a brand-new
            // transcript by `ProjectionReconciler` at the next boot.
            //
            // Retire rather than physically purge, even though this is the
            // strongest deletion a user can perform: retirement already removes
            // the events from EVERY read path (replay, BM25 search — the FTS
            // mirror is physically dropped, run markers, reconciler), so no
            // content the user believes deleted can surface again. A hard purge
            // would be a second deletion mechanism for the same job with no
            // additional user-visible effect, and it would free seqs that a
            // re-created session under the same (stable) key could collide with.
            let retired = match crate::session::store::retire_live_events(&session_key, 1).await {
                Ok(n) => n,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to delete session event log: {e}"),
                    );
                }
            };

            match manager.delete_session(&session_key).await {
                Ok(result) => {
                    // The transcript is gone; the bytes it produced must go
                    // with it. Without this the store keeps up to
                    // `MAX_ARTIFACTS_PER_SESSION` blobs per deleted session on
                    // disk forever, and a re-created session under the same
                    // (stable) key would inherit the dead session's artifacts.
                    //
                    // Canonical spelling, not the caller's `key_str`: the
                    // harvest points stamp `to_key_string()`, and the store
                    // encodes the key straight into a directory name — a
                    // legacy spelling would purge a directory that never
                    // existed and silently leave the real one behind.
                    purge_session_artifacts(&session_key.to_key_string()).await;
                    // SessionEnd — the session has been removed; extension
                    // observers witness the teardown.
                    fire_session_end_hook(&session_key).await;
                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "session_key": key_str,
                            "deleted": result.deleted || retired > 0,
                            "events_retired": retired,
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

    // Both stores merge `metadata` opaquely into `identity_meta.custom`, so this
    // is the only place an `exec_tier` written through sessions.patch can be
    // checked. An unknown id would persist, render as an override in the Panel,
    // and then be dropped with a warn at run time — every turn silently running
    // at the GLOBAL tier, possibly weaker than the one the caller believes it
    // armed. `chat.send` already refuses an unknown id (handlers/agent.rs); the
    // two write paths must agree. `null` stays legal: it is how "follow global"
    // clears the override.
    if let Some(v) = params
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|m| m.get(crate::config::types::policies::EXEC_TIER_SESSION_KEY))
    {
        let valid = v.is_null()
            || v.as_str()
                .and_then(crate::config::types::policies::ExecTier::from_id)
                .is_some();
        if !valid {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Unknown exec_tier: {v}"),
            );
        }
    }

    // Same paired-validation contract for the session usage mode (the tier's
    // third twin): `chat.send` refuses an unknown mode id in
    // `build_run_request`, so this write path must refuse it too, or a stored
    // junk value shows as an override in the Panel while every turn silently
    // runs at the global default. `null` stays legal — "follow global".
    if let Some(v) = params
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|m| m.get(crate::config::types::policies::MODE_SESSION_KEY))
    {
        let valid = v.is_null()
            || v.as_str()
                .and_then(crate::config::types::policies::SessionMode::from_id)
                .is_some();
        if !valid {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Unknown session_mode: {v}"),
            );
        }
    }

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
            crate::gateway::session_store::types::CompactStrategy::KeepLastN {
                n: crate::gateway::session_store::types::SESSION_COMPACT_KEEP_LAST_N,
            },
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

/// Handle `sessions.set_project_root` RPC request with database backend.
///
/// Params:
///   - `session_key` (required): session key string
///   - `project_root` (optional): absolute project folder; `null` / absent
///     clears the override (revert to the default agent workspace).
///
/// Persists the user-chosen working directory onto the session's identity
/// metadata so the Panel can restore it after a reload. Validation of the path
/// (absolute / exists / directory) happens at run time in the `agent.run`
/// handler — this RPC only records the preference, so a not-yet-existing folder
/// can be remembered without failing the call.
pub async fn handle_set_project_root_db(
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

    // `project_root` is optional: a missing key or JSON null clears the override.
    let project_root = params
        .get("project_root")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty());

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

    match manager.set_project_root(&session_key, project_root).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "project_root": project_root,
                "updated": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set project_root: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::session::events::{MessageContent, SessionEvent};
    use crate::session::store::SessionEventStore;
    use tempfile::tempdir;

    fn user_event(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: 0,
            synthetic: false,
        }
    }

    /// Deleting a conversation must delete the conversation — not just the
    /// transcript the Panel happens to read. The event log is the SSOT: leaving
    /// it live keeps the content replayable by the model, searchable via BM25,
    /// and re-materialisable by `ProjectionReconciler` at the next boot.
    #[tokio::test]
    async fn delete_retires_the_event_log_so_nothing_replays_or_searches() {
        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("delete_ssot.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:deltest:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        events
            .append(&key, 1, &user_event("the passphrase is hunter2"), 0)
            .await
            .unwrap();

        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.delete".into(),
            params: Some(json!({ "session_key": key.to_key_string() })),
            id: Some(json!(1)),
        };
        let response = handle_delete_db(request, store).await;
        assert!(
            response.error.is_none(),
            "delete failed: {:?}",
            response.error
        );
        let result = response.result.expect("result");
        assert_eq!(result["deleted"], true);
        assert_eq!(result["events_retired"], 1);

        assert!(
            events.load_all_events(&key).await.unwrap().is_empty(),
            "a deleted conversation must yield nothing to a replay"
        );
        assert!(
            events
                .search_events(&key, "passphrase", 5)
                .await
                .unwrap()
                .is_empty(),
            "a deleted conversation must yield nothing to FTS"
        );
    }

    /// `sessions.patch` is the tier picker's own write path. An id the run loop
    /// cannot resolve must be refused here — otherwise it persists, the Panel
    /// renders it as an armed override, and every turn silently runs at the
    /// global tier. `chat.send` already refuses one; the two write paths must
    /// agree. `null` must keep working: it is how "follow global" clears the
    /// override.
    #[tokio::test]
    async fn patch_refuses_an_unknown_exec_tier_but_accepts_null_and_a_real_tier() {
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("patch_tier.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:tierpatch:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let key_str = key.to_key_string();

        let patch = |tier: Value| JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.patch".into(),
            params: Some(json!({
                "session_key": key_str,
                "metadata": { "exec_tier": tier },
            })),
            id: Some(json!(1)),
        };

        let rejected = handle_patch_db(patch(json!("strict")), Arc::clone(&store)).await;
        assert!(
            rejected.error.is_some(),
            "an unknown tier id must not persist"
        );

        for accepted in [json!("ask"), Value::Null] {
            let response = handle_patch_db(patch(accepted.clone()), Arc::clone(&store)).await;
            assert!(
                response.error.is_none(),
                "`{accepted}` is a legal exec_tier write: {:?}",
                response.error
            );
        }
    }
}
