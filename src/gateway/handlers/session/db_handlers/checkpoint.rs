//! Checkpoint handlers for session compaction.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

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

    let meta = match manager.get_metadata(&key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — a foreign session's checkpoint
        // existence must not be observable either.
        return visibility::not_found_response(request.id);
    }

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

    let meta = match manager.get_metadata(&key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — before overwriting the foreign
        // session's live transcript with stale checkpoint content.
        return visibility::not_found_response(request.id);
    }

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

    // Source (addressed) session: standard KeyChecked pattern.
    let meta = match manager.get_metadata(&key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — this is the read-compromise case:
        // `branch_from_checkpoint` copies the source session's full verbatim
        // checkpoint messages into `new_key`, so letting a foreign source
        // through here would hand the caller someone else's conversation.
        return visibility::not_found_response(request.id);
    }

    // Target collision: `branch_from_checkpoint` writes directly to
    // `new_key` with no existence check of its own (confirmed by reading
    // both backends) — an attacker-chosen `new_session_key` that already
    // names a session they don't own would otherwise be silently
    // overwritten with the branched content. There is no pre-existing
    // "target already exists" error to reuse here (the store never checked
    // this), so this reuses the SAME oracle-safe response the source check
    // above uses, rather than inventing a new one — a collision with a
    // foreign session reads identically to a missing/foreign source.
    match manager.get_metadata(&new_key).await {
        Ok(Some(existing)) if !visibility::session_visible(&existing) => {
            return visibility::not_found_response(request.id);
        }
        Ok(_) => {}
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    }

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

/// P1 visibility chokepoint — pinned per team-lead fix round 1. Checkpoints
/// are file-backend only (the SQLite backend returns `Unsupported` for all
/// three ops), so these tests use `FileSessionStore`. None of them need a
/// REAL checkpoint on disk: the visibility check runs before
/// `list_checkpoints`/`restore_checkpoint`/`branch_from_checkpoint` are ever
/// called, so a denial fires regardless of whether the addressed session has
/// any checkpoints at all.
#[cfg(test)]
mod visibility_guards {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::protocol::RESOURCE_NOT_FOUND;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::scope::{with_scope, ScopeAttribution};
    use tempfile::TempDir;

    fn store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    async fn owned_session(store: &Arc<dyn SessionStore>, agent: &str, owner: &str) -> SessionKey {
        let key = SessionKey::from_key_string(&format!("agent:{agent}:main")).unwrap();
        with_scope(
            Some(ScopeAttribution::personal(owner)),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        key
    }

    #[tokio::test]
    async fn compaction_list_denies_a_foreign_session_key() {
        let temp = TempDir::new().unwrap();
        let store = store(&temp);
        let alice_key = owned_session(&store, "alicecplist", "u-alice").await;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.compaction.list".into(),
            params: Some(json!({ "session_key": alice_key.to_key_string() })),
            id: Some(json!(1)),
        };
        let as_bob = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_list_checkpoints_db(req, store.clone()),
            )
            .await;
        assert_eq!(
            as_bob.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn compaction_restore_denies_a_foreign_session_key() {
        let temp = TempDir::new().unwrap();
        let store = store(&temp);
        let alice_key = owned_session(&store, "alicecprestore", "u-alice").await;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.compaction.restore".into(),
            params: Some(json!({
                "session_key": alice_key.to_key_string(),
                "checkpoint_id": "cp-1",
            })),
            id: Some(json!(1)),
        };
        let as_bob = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_restore_checkpoint_db(req, store.clone()),
            )
            .await;
        assert_eq!(
            as_bob.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn compaction_branch_denies_a_foreign_source_session() {
        let temp = TempDir::new().unwrap();
        let store = store(&temp);
        let alice_key = owned_session(&store, "alicecpbranchsrc", "u-alice").await;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.compaction.branch".into(),
            params: Some(json!({
                "session_key": alice_key.to_key_string(),
                "checkpoint_id": "cp-1",
                "new_session_key": "agent:bobsnewbranch:main",
            })),
            id: Some(json!(1)),
        };
        let as_bob = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_branch_checkpoint_db(req, store.clone()),
            )
            .await;
        assert_eq!(
            as_bob.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        // The proposed new session must never have been created — a denied
        // branch must have no side effect either.
        let new_key = SessionKey::from_key_string("agent:bobsnewbranch:main").unwrap();
        assert!(store.get_metadata(&new_key).await.unwrap().is_none());
    }

    /// The read-compromise shape team-lead review flagged: bob owns the
    /// SOURCE (so that check passes) but points `new_session_key` at
    /// alice's EXISTING session. Denied at the target-collision check,
    /// before `branch_from_checkpoint` ever runs — alice's session must
    /// come through completely unmodified (still hers, still whatever it
    /// was before).
    #[tokio::test]
    async fn compaction_branch_denies_a_foreign_target_collision_and_leaves_it_intact() {
        let temp = TempDir::new().unwrap();
        let store = store(&temp);
        let bob_key = owned_session(&store, "bobcpbranchsrc", "u-bob").await;
        let alice_key = owned_session(&store, "alicecpbranchtarget", "u-alice").await;
        let alice_before = store.get_metadata(&alice_key).await.unwrap().unwrap();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.compaction.branch".into(),
            params: Some(json!({
                "session_key": bob_key.to_key_string(),
                "checkpoint_id": "cp-1",
                "new_session_key": alice_key.to_key_string(),
            })),
            id: Some(json!(1)),
        };
        let as_bob = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_branch_checkpoint_db(req, store.clone()),
            )
            .await;
        assert_eq!(
            as_bob.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        let alice_after = store.get_metadata(&alice_key).await.unwrap().unwrap();
        assert_eq!(
            alice_after.owner_user_id, alice_before.owner_user_id,
            "a denied branch must not overwrite the target session's ownership"
        );
        assert_eq!(
            alice_after.created_at, alice_before.created_at,
            "a denied branch must not overwrite the target session at all"
        );
    }

    /// A non-colliding target for the caller's OWN source must not be
    /// blocked by the visibility gate itself — it fails later, for the
    /// unrelated (pre-existing, expected) reason that no real checkpoint
    /// exists, proving the gate doesn't swallow the legitimate case.
    #[tokio::test]
    async fn compaction_branch_own_source_passes_the_visibility_gate() {
        let temp = TempDir::new().unwrap();
        let store = store(&temp);
        let bob_key = owned_session(&store, "bobcpbranchown", "u-bob").await;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.compaction.branch".into(),
            params: Some(json!({
                "session_key": bob_key.to_key_string(),
                "checkpoint_id": "cp-does-not-exist",
                "new_session_key": "agent:bobbranchfresh:main",
            })),
            id: Some(json!(1)),
        };
        let as_bob = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_branch_checkpoint_db(req, store.clone()),
            )
            .await;
        assert_ne!(
            as_bob.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND),
            "the visibility gate must not block the caller's own session"
        );
    }
}
