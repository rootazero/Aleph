//! Session creation handlers.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

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
///   - `session_key` (required): current session key string
///   - topic (optional): topic for the closing session (if omitted, no topic is stored)
///
/// Returns:
///   - `old_session_key`: the closed session key
///   - `new_session_key`: the newly created session key (epoch incremented)
///   - topic: the topic stored (if any)
pub async fn handle_new_session_db(
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

    let topic = request
        .params
        .as_ref()
        .and_then(|p| p.get("topic"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // ONE parse of the caller's string, reused by every step below.
    //
    // This used to be parsed twice — `from_key_string` here and
    // `SessionKey::parse` again before the epoch bump. They are the same type
    // and agree whenever `parse` succeeds, but `from_key_string` is
    // `parse().or_else(from_legacy)`, so a legacy-only key (e.g.
    // `agent:x:peer:a:b`) passed the first gate and failed the second — after
    // the close and the side-session retirement had already run. Two parses of
    // one string is two answers to "which key is this", and the retirement
    // derives its target from that answer.
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

    let meta = match manager.get_metadata(&legacy_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — before any of the mutations below:
        // closing a foreign session and killing its continuations must not
        // happen just because the caller can guess/enumerate its key.
        return visibility::not_found_response(request.id);
    }

    // Terminate the closing session's autonomous continuations BEFORE the
    // epoch bump — a loop/goal keyed under the old epoch would otherwise keep
    // its self-sustaining chain alive with no session left that can stop it
    // (same seam as the channel `/new` command). The same call retires the
    // `/btw` side session derived from this key, which the epoch bump is about
    // to make unreachable.
    crate::gateway::continuation_lifecycle::terminate_session_continuations(
        &legacy_key,
        "sessions.new",
        Some(manager.clone()),
    );

    // Close old session
    if let Err(e) = manager.close_session(&legacy_key, topic.as_deref()).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to close session: {e}"),
        );
    }

    // Create new epoch key from the single parse above.
    let new_routing_key = legacy_key.with_next_epoch();
    let new_key_str = new_routing_key.to_key_string();

    // Create the new session UNDER the closing session's attribution.
    //
    // `stamp_attribution` reads the ambient scope in `get_or_create`'s CREATE
    // branch, and the gateway dispatch loop has none — so without this the
    // successor row was written NULL/NULL and adopted by `owner_or_legacy`. For
    // a personal session that silently transferred it to the operator; for a
    // ROOM it was worse, because the new row no longer declared itself a room
    // at all, so the roster had nothing to intervene on and every member lost
    // the conversation they were in.
    //
    // The source attribution is sitting in `meta`, read four statements above
    // for the visibility gate. `from_persisted` returns `None` for a legacy
    // (unstamped) source, which reproduces today's behaviour exactly — this
    // only ever carries an attribution that already existed.
    //
    // ⚠️ Load-bearing that this happens at CREATE time: `stamp_attribution`
    // early-returns on an already-stamped row, so a successor written with the
    // wrong owner is permanent and no later backfill can see it.
    let carried = crate::scope::ScopeAttribution::from_persisted(
        meta.owner_user_id.as_deref(),
        meta.scope_id.as_deref(),
    );
    match crate::scope::with_scope(carried, manager.get_or_create(&new_routing_key)).await {
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

/// P1 visibility chokepoint — pinned per team-lead fix round 1.
#[cfg(test)]
mod visibility_guards {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::protocol::RESOURCE_NOT_FOUND;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::scope::{with_scope, ScopeAttribution};
    use tempfile::TempDir;

    fn store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("new_session_visibility.db"),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    /// `sessions.new` closes the addressed session and kills its autonomous
    /// continuations — a denied call must do neither. "Intact" here means
    /// the state observed before the call is bit-for-bit what's observed
    /// after (no close, no epoch bump consumed).
    #[tokio::test]
    async fn sessions_new_denies_a_foreign_session_and_leaves_it_intact() {
        let temp = TempDir::new().unwrap();
        let store = store(&temp);
        let alice_key = SessionKey::from_key_string("agent:alicenewvis:main").unwrap();
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&alice_key),
        )
        .await
        .unwrap();
        let before = store.get_metadata(&alice_key).await.unwrap().unwrap();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.new".into(),
            params: Some(json!({ "session_key": alice_key.to_key_string() })),
            id: Some(json!(1)),
        };
        let as_bob = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_new_session_db(req, store.clone()),
            )
            .await;
        assert_eq!(
            as_bob.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );

        let after = store.get_metadata(&alice_key).await.unwrap().unwrap();
        assert_eq!(
            after.state, before.state,
            "a denied sessions.new must not close the foreign session"
        );
    }
}
