//! Subagent tree handler — read-only snapshot of the background sub-agent tree.
//!
//! `subagent.tree` returns the **flat** background sub-agent nodes from the
//! process-global `BackgroundAgentTracker`, optionally filtered to one root
//! session. The panel rebuilds the hierarchy with `aleph_protocol::build_tree`
//! — the same shared reconstruction it runs on each live `run.subagent_tree`
//! delta, so cold-start and live paths are byte-identical (one Rust tree
//! builder, compiled to WASM; no Python+TS-style double implementation).
//! Pure I/O (R4/R10) — no reasoning, no mutation.
//!
//! ## Request
//! ```json
//! { "root_session": "agent:session-key" }   // optional; omitted = whole process
//! ```
//!
//! ## Response (success)
//! ```json
//! { "nodes": [ ...SubagentNode... ], "count": 3 }
//! ```

use std::collections::HashMap;

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::router::SessionKey;
use crate::agents::background_tracker::BackgroundAgentTracker;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::sync_primitives::Arc;

/// `subagent.tree` — snapshot the flat background sub-agent nodes for the panel.
///
/// P1 (spec §11-1c): `sessions` backs two visibility checks.
/// - `root_session` given → the Task-6 addressed-key pattern: resolve it and
///   deny with `visibility::not_found_response` unless visible.
/// - `root_session` omitted → an unrestricted caller keeps the pre-P1
///   whole-process view unchanged; a scoped caller gets the SAME flat list
///   filtered to nodes whose `root_session` is visible (never an error — an
///   empty tree, same as an unrestricted caller with no subagents running,
///   is a valid answer).
pub async fn handle_tree(
    request: JsonRpcRequest,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let root_session = request
        .params
        .as_ref()
        .and_then(|p| p.get("root_session"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let flat = match root_session {
        Some(raw) => {
            let key = match SessionKey::from_key_string(&raw) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid root_session format",
                    )
                }
            };
            match sessions.get_metadata(&key).await {
                Ok(Some(meta)) if visibility::session_visible(&meta) => {}
                _ => return visibility::not_found_response(request.id),
            }
            BackgroundAgentTracker::global().flat_nodes(Some(&key.to_key_string()))
        }
        None => {
            let all = BackgroundAgentTracker::global().flat_nodes(None);
            if visibility::visible_owner_filter().is_none() {
                all // Unrestricted caller: whole-process view, unchanged.
            } else {
                // Memoize per distinct root_session so a tree with many nodes
                // under the same root pays one SessionStore lookup, not N.
                let mut resolved: HashMap<String, bool> = HashMap::new();
                let mut filtered = Vec::with_capacity(all.len());
                for node in all {
                    let visible = match resolved.get(&node.root_session) {
                        Some(&v) => v,
                        None => {
                            let v = match SessionKey::from_key_string(&node.root_session) {
                                Some(key) => matches!(
                                    sessions.get_metadata(&key).await,
                                    Ok(Some(meta)) if visibility::session_visible(&meta)
                                ),
                                None => false,
                            };
                            resolved.insert(node.root_session.clone(), v);
                            v
                        }
                    };
                    if visible {
                        filtered.push(node);
                    }
                }
                filtered
            }
        }
    };
    let count = flat.len();

    match serde_json::to_value(&flat) {
        Ok(nodes) => {
            JsonRpcResponse::success(request.id, json!({ "nodes": nodes, "count": count }))
        }
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("subagent.tree serialize failed: {err}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::background_tracker::{CompletedOutcome, SpawnMeta};
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn sessions() -> (TempDir, Arc<dyn SessionStore>) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: tmp.path().to_path_buf(),
                ..Default::default()
            })
            .expect("file session store"),
        );
        (tmp, store)
    }

    async fn create_session(
        sessions: &Arc<dyn SessionStore>,
        session_key: &str,
        owner: Option<&str>,
    ) {
        let key = SessionKey::from_key_string(session_key).expect("valid session_key fixture");
        let attribution = owner.map(crate::scope::ScopeAttribution::personal);
        crate::scope::with_scope(attribution, sessions.get_or_create(&key))
            .await
            .expect("get_or_create");
    }

    /// A session_key unique to this test call, so an assertion on
    /// `flat_nodes(Some(root))`'s exact node count (or on the *entire*
    /// `flat_nodes(None)` snapshot) cannot pick up leftover completed
    /// entries `mark_completed` leaves queryable (non-destructively, until
    /// TTL prune — see that method's doc) from OTHER tests sharing the same
    /// process-global `BackgroundAgentTracker` singleton.
    fn unique_session_key() -> String {
        format!("agent:main:t-{}", uuid::Uuid::new_v4().simple())
    }

    /// Register a background agent under `root_session` on the process-global
    /// tracker, unregistering it again on drop so tests don't leak state into
    /// each other (the tracker is a `OnceLock` singleton, shared process-wide).
    struct RegisteredAgent {
        request_id: String,
    }

    impl Drop for RegisteredAgent {
        fn drop(&mut self) {
            BackgroundAgentTracker::global()
                .mark_completed(&self.request_id, CompletedOutcome::ok_text("test cleanup"));
        }
    }

    fn register(request_id: &str, root_session: &str) -> RegisteredAgent {
        BackgroundAgentTracker::global().register_with_meta(
            request_id.to_string(),
            CancellationToken::new(),
            "a task".to_string(),
            SpawnMeta {
                parent_id: None,
                depth: 1,
                root_session: root_session.to_string(),
                model: None,
                child_session: None,
            },
        );
        RegisteredAgent {
            request_id: request_id.to_string(),
        }
    }

    fn tree_request(root_session: Option<&str>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "subagent.tree".to_string(),
            params: root_session.map(|r| json!({ "root_session": r })),
            id: Some(json!(1)),
        }
    }

    /// `root_session` given, visible: the addressed-key pattern lets it
    /// through unchanged (pre-P1 behaviour for a session the caller owns).
    #[tokio::test]
    async fn addressed_root_session_visible_returns_its_nodes() {
        let root = unique_session_key();
        let (_tmp, sess) = sessions();
        create_session(&sess, &root, Some("u-alice")).await;
        let _agent = register(&format!("req-{root}"), &root);

        let resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_tree(tree_request(Some(&root)), sess).await
            })
            .await;
        let result = resp.result.expect("success");
        assert_eq!(result["count"], 1);
    }

    /// P1: `root_session` given but owned by someone else — NOT_FOUND, same
    /// shape a genuinely unknown root_session would produce (no oracle).
    #[tokio::test]
    async fn addressed_root_session_denies_a_foreign_owner() {
        let root = unique_session_key();
        let (_tmp, sess) = sessions();
        create_session(&sess, &root, Some("u-alice")).await;
        let _agent = register(&format!("req-{root}"), &root);

        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_tree(tree_request(Some(&root)), sess).await
            })
            .await;
        assert_eq!(
            resp.error.expect("expected error").code,
            crate::gateway::protocol::RESOURCE_NOT_FOUND
        );
    }

    /// Omitted `root_session`, unrestricted caller: whole-process view is
    /// unchanged from pre-P1 — the zero-change guarantee for internal/cron/
    /// single-user callers every predicate in `gateway::visibility` upholds.
    /// Asserted by checking the two fresh roots THIS test registered are
    /// both present, not by an exact total (the tracker is process-global
    /// and shared with every other test in this module).
    #[tokio::test]
    async fn omitted_root_session_unrestricted_keeps_whole_process_view() {
        let root_a = unique_session_key();
        let root_b = unique_session_key();
        let (_tmp, sess) = sessions();
        create_session(&sess, &root_a, Some("u-alice")).await;
        create_session(&sess, &root_b, Some("u-bob")).await;
        let _a = register(&format!("req-{root_a}"), &root_a);
        let _b = register(&format!("req-{root_b}"), &root_b);

        let resp = handle_tree(tree_request(None), sess).await;
        let result = resp.result.expect("success");
        let nodes = result["nodes"].as_array().expect("nodes array");
        let roots: std::collections::HashSet<&str> = nodes
            .iter()
            .filter_map(|n| n["root_session"].as_str())
            .collect();
        assert!(
            roots.contains(root_a.as_str()) && roots.contains(root_b.as_str()),
            "unrestricted caller sees every root, including both fresh ones: {roots:?}"
        );
    }

    /// P1's own acceptance case: omitted `root_session` as a member lists
    /// only trees whose session that member owns — never an error, an empty
    /// (or partial) tree is a valid answer.
    #[tokio::test]
    async fn omitted_root_session_scoped_caller_sees_only_own_trees() {
        let alice_root = unique_session_key();
        let bob_root = unique_session_key();
        let (_tmp, sess) = sessions();
        create_session(&sess, &alice_root, Some("u-alice")).await;
        create_session(&sess, &bob_root, Some("u-bob")).await;
        let _alice = register(&format!("req-{alice_root}"), &alice_root);
        let _bob = register(&format!("req-{bob_root}"), &bob_root);

        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_tree(tree_request(None), sess).await
            })
            .await;
        let result = resp.result.expect("success, not an error");
        let nodes = result["nodes"].as_array().expect("nodes array");
        assert!(
            nodes.iter().any(|n| n["root_session"] == bob_root.as_str()),
            "bob's own tree must be present: {nodes:?}"
        );
        assert!(
            !nodes
                .iter()
                .any(|n| n["root_session"] == alice_root.as_str()),
            "alice's tree must be omitted: {nodes:?}"
        );
    }
}
