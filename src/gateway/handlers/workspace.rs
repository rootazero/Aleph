//! Workspace RPC Handlers
//!
//! Handlers for workspace management: create, list, get, update, archive.
//! Channel agent binding: `channels.set_agent`, agents.bindings.
//! All handlers delegate to `AgentEnvStore` (SQLite-backed).

use aleph_protocol::workspace::{WorkspaceCreateParams, WorkspaceRef, WorkspaceUpdateParams};
use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use super::parse_params;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;

// ============================================================================
// Create
// ============================================================================

/// Create a new workspace
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.create","params":{"id":"crypto","name":"Crypto Trading"},"id":1}
/// ```
///
/// The param type is [`aleph_protocol::workspace::WorkspaceCreateParams`], the
/// same struct the CLI constructs — see that module for why the shape is not
/// declared here, and for how this method came to reject its only client on
/// every call while every test stayed green.
///
/// P1 partition isolation on the WRITE side, with exactly the coverage
/// boundary [`handle_list`] documents and no more: a workspace id is a
/// user-chosen name that encodes no owner and `agent_envs` has no owner
/// column, so an ordinary id (`"crypto"`) passes for every caller who reaches
/// this handler. What the check does buy is the composed-id half — without it
/// a caller can create `main__u-alice`, a row that then shows up in ALICE's
/// filtered `workspace.list` carrying attacker-supplied `env_vars` /
/// `system_prompt_override` / `allowed_tools`. Reads were gated first; a
/// write into a partition you cannot read is the strictly worse half.
///
/// Since 2026-08-08 the reachable caller set is operator-only — the whole
/// `workspace.` family is admin-gated ([`handle_list`]'s doc has the finding
/// that made that the right fix rather than an owner column). The predicate
/// stays because it is not vacuous for an operator either.
///
/// The denial reuses this method's own "that id is not available" shape,
/// produced from the REAL [`crate::gateway::agent_env::AgentEnvError::AlreadyExists`]
/// value so it stays
/// byte-identical to a genuine collision instead of hard-coding a copy of the
/// store's wording.
pub async fn handle_create(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceCreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !crate::gateway::visibility::partition_visible(&params.id) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to create workspace: {}",
                crate::gateway::agent_env::AgentEnvError::AlreadyExists(params.id.clone())
            ),
        );
    }

    match workspace_manager
        .create(&params.id, "default", params.description.as_deref())
        .await
    {
        Ok(mut ws) => {
            // Apply name and icon which create() doesn't accept directly
            ws.name = params.name;
            ws.icon = params.icon.clone();

            // Persist name/icon via update
            if let Err(e) = workspace_manager
                .update(&params.id, Some(&ws.name), None, params.icon.as_deref())
                .await
            {
                tracing::warn!(
                    workspace = %params.id,
                    error = %e,
                    "workspace.create: failed to persist name/icon after create"
                );
            }

            JsonRpcResponse::success(
                request.id,
                json!({
                    "ok": true,
                    "workspace": ws,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create workspace: {e}"),
        ),
    }
}

// ============================================================================
// List
// ============================================================================

/// List all workspaces
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.list","id":1}
/// ```
///
/// # P1 partition isolation — and what it does NOT cover
///
/// A workspace serializes as an [`AgentEnv`], which carries `env_vars`,
/// `system_prompt_override` and `allowed_tools`. Each row is filtered by
/// `visibility::partition_visible` on its id, the same predicate the
/// `memory.*`/`graph.*` family uses, so an id composed with the partition
/// grammar (`<base>__u-alice`) is invisible to everyone else.
///
/// That is defense in depth, NOT a closed boundary on its own, and the
/// distinction is recorded here rather than implied by the presence of a
/// check: a workspace id is a user-chosen name (`"project-aleph"`), it
/// encodes no owner, and the `agent_envs` table has no owner column — so an
/// ordinary workspace passes this predicate for every caller who reaches this
/// handler.
///
/// # The residual was write, and it is closed at the admin gate
///
/// The 2026-08-08 real-machine QA exercised the write half: a member renamed
/// and then archived a workspace the operator had just created, both
/// returning `ok`. An earlier wording here named only "one member can read
/// another's `env_vars`", which understated it by a whole verb class —
/// `update` and `archive` take the same plain id and clear the same predicate.
/// (That QA also produced a *false* pass on the list side: this handler looked
/// filtered only because the member had archived the row one call earlier and
/// `list(false)` skips archived.)
///
/// That wording also said closing it needed an owner column plus a migration,
/// "a schema change and a product decision, not a handler fix". It needed
/// neither. The whole `workspace.` family joined
/// [`crate::gateway::method_admin`]'s `ADMIN_PREFIXES` on 2026-08-08, with no
/// carve-out, because the family has exactly one client and it is already
/// operator (`aleph workspace list|create|archive`, over loopback); the Panel
/// has none, and `update`/`get` have no client anywhere. A member no longer
/// reaches any method in this file.
///
/// **The predicate below still earns its place, and this file is now the only
/// place that says so.** The family left
/// `method_visibility::SCOPED_METHODS` in the same change — that table's
/// contract is per-user filtering on surfaces a MEMBER reaches, so a claim
/// there would have gone stale — but leaving the table is not the handlers
/// dropping the check. A second `UserRole::Admin` principal connects with
/// `CALLER_USER = Some(their own id)` rather than `OWNER_USER_ID`
/// (`handlers::connect::resolve_connection_identity` returns the linked
/// user's id for any admin that is not the zero-config loopback owner), so
/// `partition_visible` still refuses `main__u-alice` to an operator who is
/// not alice. Two gates, two questions — "may this role call it" and "may
/// this caller address that partition" — and neither implies the other.
pub async fn handle_list(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    match workspace_manager.list(false).await {
        Ok(workspaces) => {
            let visible: Vec<_> = workspaces
                .into_iter()
                .filter(|w| crate::gateway::visibility::partition_visible(&w.id))
                .collect();
            JsonRpcResponse::success(request.id, json!({ "workspaces": visible }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list workspaces: {e}"),
        ),
    }
}

// ============================================================================
// Get
// ============================================================================

/// Get a workspace by ID
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.get","params":{"id":"crypto"},"id":1}
/// ```
///
/// P1 partition isolation, with the same coverage boundary [`handle_list`]
/// documents: an invisible partition-composed id gets this method's OWN
/// "not found" response, byte-identical to an id that does not exist, and
/// the store is not read for it.
pub async fn handle_get(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager.get(&params.id).await {
        Ok(Some(ws)) => JsonRpcResponse::success(request.id, json!({ "workspace": ws })),
        Ok(None) => not_found(),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get workspace: {e}"),
        ),
    }
}

// ============================================================================
// Update
// ============================================================================

/// Update workspace metadata
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.update","params":{"id":"crypto","name":"Crypto Research"},"id":1}
/// ```
///
/// P1 partition isolation, same predicate and the SAME coverage boundary
/// [`handle_create`] and [`handle_list`] spell out — defense in depth against
/// partition-composed ids, not a closed boundary for ordinary ones. An
/// invisible id gets this method's OWN "not found" response, byte-identical to
/// an id that does not exist, and the store is never written for it.
pub async fn handle_update(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceUpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager
        .update(
            &params.id,
            params.name.as_deref(),
            params.description.as_deref(),
            params.icon.as_deref(),
        )
        .await
    {
        Ok(Some(ws)) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "workspace": ws,
            }),
        ),
        Ok(None) => not_found(),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update workspace: {e}"),
        ),
    }
}

// ============================================================================
// Archive
// ============================================================================

/// Archive (soft-delete) a workspace
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.archive","params":{"id":"crypto"},"id":1}
/// ```
///
/// P1 partition isolation, same predicate and the SAME coverage boundary
/// [`handle_create`] and [`handle_list`] spell out. The soft-delete is the
/// most destructive verb in this file — an invisible id gets this method's
/// OWN "not found" response, byte-identical to an id that does not exist, and
/// the row is never archived.
pub async fn handle_archive(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager.archive(&params.id).await {
        Ok(true) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Ok(false) => not_found(),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to archive workspace: {e}"),
        ),
    }
}

// ============================================================================
// Channel Agent Binding (1:1 model)
// ============================================================================

/// Parameters for `channels.set_agent`
#[derive(Debug, Deserialize)]
pub struct SetAgentParams {
    pub channel_id: String,
    pub agent_id: Option<String>,
}

/// Bind or unbind an agent to/from a channel.
///
/// If `agent_id` is Some, binds the agent (with 1:1 constraint).
/// If `agent_id` is None, unbinds the current agent.
///
/// Delegates to the shared binding seam (`gateway::agent_binding`) — the same
/// implementation behind the `agent_switch` tool — so both surfaces share
/// ghost validation, no-op detection, and `Bound`/`Unbound` lifecycle events
/// (this RPC previously emitted no event at all, leaving other Panels stale).
/// `agent_registry: None` (a minimal server) skips validation, preserving the
/// prior unchecked behavior rather than blocking the bind.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"channels.set_agent","params":{"channel_id":"rpc","agent_id":"project-x"},"id":1}
/// ```
pub async fn handle_set_agent(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
    agent_registry: Option<Arc<AgentRegistry>>,
    event_bus: Option<Arc<GatewayEventBus>>,
) -> JsonRpcResponse {
    use crate::gateway::agent_binding::{bind_channel_agent, unbind_channel_agent, BindError};

    let params: SetAgentParams =
        match serde_json::from_value(request.params.clone().unwrap_or_default()) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
        };

    match params.agent_id {
        Some(agent_id) => {
            match bind_channel_agent(
                agent_registry.as_deref(),
                &workspace_manager,
                event_bus.as_deref(),
                &params.channel_id,
                &agent_id,
            )
            .await
            {
                Ok(outcome) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "ok": true,
                        "previous_agent": outcome.previous_agent,
                        "no_op": outcome.no_op,
                    }),
                ),
                Err(e @ (BindError::UnknownAgent { .. } | BindError::EmptyChannel)) => {
                    JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string())
                }
                Err(e @ BindError::Store(_)) => {
                    JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string())
                }
            }
        }
        None => {
            match unbind_channel_agent(&workspace_manager, event_bus.as_deref(), &params.channel_id)
            {
                Ok(previous) => JsonRpcResponse::success(
                    request.id,
                    json!({"ok": true, "previous_agent": previous}),
                ),
                Err(e @ BindError::EmptyChannel) => {
                    JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string())
                }
                Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
            }
        }
    }
}

/// Get every channel bound to each agent for the Panel (many-to-one aware).
///
/// Response shape: `{"bindings": {"<agent_id>": ["<channel>", …]}}` — channels
/// sorted. The previous one-channel-per-agent map was lossy (an agent bound to
/// several channels showed only one); consumers were migrated with the shape.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"agents.bindings","id":1}
/// ```
pub async fn handle_agent_bindings(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    match workspace_manager.bindings_by_agent() {
        Ok(bindings) => JsonRpcResponse::success(request.id, json!({"bindings": bindings})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Final-review I6, defense-in-depth half: a partition-composed workspace
    /// id belonging to another user is invisible, and `get` denies with this
    /// method's own not-found rather than a distinct error.
    ///
    /// What this test deliberately does NOT claim: that ordinary workspaces
    /// are isolated. `"crypto"` carries no owner, so it passes the predicate
    /// for everyone who reaches this handler — that half is answered by the
    /// admin gate instead (see [`handle_list`]'s doc).
    ///
    /// The scoped id below is written as a member's for readability, but this
    /// calls the handler directly and so proves nothing about who may dispatch
    /// to it: since 2026-08-08 a member cannot — `workspace.` is admin-gated,
    /// pinned in [`crate::gateway::method_admin`]'s own tests. The predicate
    /// under test binds operators too: a second `UserRole::Admin` principal
    /// carries its OWN `CALLER_USER`, not `OWNER_USER_ID`, so this is the
    /// surviving case rather than a hypothetical one.
    #[tokio::test]
    async fn get_denies_a_foreign_partition_composed_id_as_not_found() {
        use crate::gateway::caller_identity::CALLER_USER;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
                archive_after_days: 0,
            })
            .expect("agent env store"),
        );
        let req = |id: &str| {
            JsonRpcRequest::with_id("workspace.get", Some(json!({ "id": id })), json!(1))
        };

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(req("main__u-alice"), store.clone()),
            )
            .await;
        let missing = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(req("main__u-alice"), store.clone()),
            )
            .await;
        assert!(denied.result.is_none(), "no AgentEnv may be serialized");
        assert_eq!(
            denied.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert_eq!(
            serde_json::to_string(&denied).unwrap(),
            serde_json::to_string(&missing).unwrap(),
            "a denied id and a nonexistent id must be byte-identical"
        );

        // Not a false positive: bob's own composed id and an ordinary
        // uncomposed id both get past the predicate (and then legitimately
        // 404, since nothing was created).
        for id in ["main__u-bob", "crypto"] {
            let resp = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_get(req(id), store.clone()),
                )
                .await;
            assert_eq!(
                resp.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND),
                "{id} should reach the store and report a genuine miss"
            );
        }
    }

    /// The write half of the same defense-in-depth check the reads carry.
    /// Reads were gated first, which left the strictly worse half open: a
    /// caller could CREATE `main__u-alice`, and that row then appears only in
    /// ALICE's filtered `workspace.list`, carrying `env_vars` /
    /// `system_prompt_override` / `allowed_tools` she never wrote. (When this
    /// was written that caller could be a member; since 2026-08-08 the family
    /// is admin-gated and the surviving case is one operator addressing
    /// another user's partition.)
    ///
    /// Each assertion is on the STORE after the call, not on the response —
    /// a check that returned the right error while the write still landed
    /// would pass a response-only test.
    ///
    /// Same boundary as [`handle_list`]: this does NOT claim ordinary
    /// workspaces are isolated. The last block proves the opposite on
    /// purpose, so nobody reads this test as more than it is.
    #[tokio::test]
    async fn the_workspace_writes_deny_a_foreign_partition_composed_id() {
        use crate::gateway::caller_identity::CALLER_USER;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
                archive_after_days: 0,
            })
            .expect("agent env store"),
        );
        // `handle_create` hard-codes the `"default"` profile; without it every
        // store write below fails as ProfileNotFound and the denials would
        // look correct for the wrong reason.
        store.load_profiles(std::collections::HashMap::from([(
            "default".to_string(),
            crate::config::ProfileConfig::default(),
        )]));
        let req = |method: &str, params: serde_json::Value| {
            JsonRpcRequest::with_id(method, Some(params), json!(1))
        };
        async fn as_bob<F: std::future::Future<Output = JsonRpcResponse>>(
            fut: F,
        ) -> JsonRpcResponse {
            crate::gateway::caller_identity::CALLER_USER
                .scope(Some("u-bob".to_string()), fut)
                .await
        }

        // --- create: the row must never come into existence -------------
        let created = as_bob(handle_create(
            req(
                "workspace.create",
                json!({ "id": "main__u-alice", "name": "planted" }),
            ),
            store.clone(),
        ))
        .await;
        assert!(created.result.is_none());
        assert!(
            store.get("main__u-alice").await.unwrap().is_none(),
            "a denied create must not insert the row"
        );

        // Byte-identical to a genuine id collision on the SAME id — the only
        // "you cannot have this id" answer this method has ever had. Alice
        // passes the predicate, so hers is the real store error.
        store
            .create("main__u-alice", "default", None)
            .await
            .unwrap();
        let collided = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_create(
                    req(
                        "workspace.create",
                        json!({ "id": "main__u-alice", "name": "planted" }),
                    ),
                    store.clone(),
                ),
            )
            .await;
        assert_eq!(
            serde_json::to_string(&created).unwrap(),
            serde_json::to_string(&collided).unwrap(),
            "the denial must be the collision shape, not a new one"
        );

        // --- update: alice's real row must keep its name ----------------
        store
            .update("main__u-alice", Some("alice's env"), None, None)
            .await
            .unwrap();
        let updated = as_bob(handle_update(
            req(
                "workspace.update",
                json!({ "id": "main__u-alice", "name": "renamed-by-bob" }),
            ),
            store.clone(),
        ))
        .await;
        assert_eq!(
            updated.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert_eq!(
            store.get("main__u-alice").await.unwrap().unwrap().name,
            "alice's env",
            "a denied update must not rewrite the foreign workspace"
        );

        // --- archive: the row must still be live ------------------------
        let archived = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "main__u-alice" })),
            store.clone(),
        ))
        .await;
        assert_eq!(
            archived.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert!(
            store.get("main__u-alice").await.unwrap().is_some(),
            "a denied archive must not soft-delete the foreign workspace"
        );
        // …and no existence oracle: the refusal must read the same whether
        // the row is there or not. `main__u-carol` is archived by bob twice —
        // once while it has never been created, once after it has — so what
        // varies between the two responses is EXISTENCE and nothing else.
        //
        // The id is deliberately held fixed rather than comparing two
        // different ids: this method's not-found message echoes the id the
        // caller itself supplied, so two ids would differ on the wire for a
        // reason that leaks nothing, and the comparison would have to be
        // weakened to survive it. Two calls that differ in neither id nor
        // store state would instead compare a call to itself and could not
        // fail at all.
        let never_created = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "main__u-carol" })),
            store.clone(),
        ))
        .await;
        store
            .create("main__u-carol", "default", None)
            .await
            .unwrap();
        let now_exists = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "main__u-carol" })),
            store.clone(),
        ))
        .await;
        assert_eq!(
            serde_json::to_string(&never_created).unwrap(),
            serde_json::to_string(&now_exists).unwrap(),
            "the refusal must not tell bob whether main__u-carol exists"
        );
        assert!(
            store.get("main__u-carol").await.unwrap().is_some(),
            "…and neither denied archive may soft-delete the row"
        );

        // --- the boundary, asserted rather than implied ------------------
        // An ordinary id encodes no owner, so bob passes the predicate for
        // it. This check is defense in depth against composed ids ONLY;
        // closing the rest needs an owner column (see `handle_list`).
        store.create("crypto", "default", None).await.unwrap();
        let ordinary = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "crypto" })),
            store.clone(),
        ))
        .await;
        assert!(
            ordinary.error.is_none(),
            "an ordinary workspace is NOT protected by this check: {:?}",
            ordinary.error
        );
    }

    #[test]
    fn test_create_params_deserialization() {
        let json = serde_json::json!({"id": "crypto", "name": "Crypto Trading"});
        let params: WorkspaceCreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name, "Crypto Trading");
        assert!(params.description.is_none());
    }

    #[test]
    fn test_create_params_with_optional_fields() {
        let json = serde_json::json!({"id": "novel", "name": "Novel", "description": "My novel project", "icon": "\u{1F4D6}"});
        let params: WorkspaceCreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.description.as_deref(), Some("My novel project"));
        assert_eq!(params.icon.as_deref(), Some("\u{1F4D6}"));
    }

    #[test]
    fn test_get_params_deserialization() {
        let json = serde_json::json!({"id": "crypto"});
        let params: WorkspaceRef = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
    }

    #[test]
    fn test_update_params_deserialization() {
        let json = serde_json::json!({"id": "crypto", "name": "Crypto Research"});
        let params: WorkspaceUpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name.as_deref(), Some("Crypto Research"));
        assert!(params.description.is_none());
        assert!(params.icon.is_none());
    }

    #[test]
    fn test_update_params_all_fields() {
        let json = serde_json::json!({
            "id": "crypto",
            "name": "Crypto Research",
            "description": "Updated description",
            "icon": "\u{1F4B0}"
        });
        let params: WorkspaceUpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name.as_deref(), Some("Crypto Research"));
        assert_eq!(params.description.as_deref(), Some("Updated description"));
        assert_eq!(params.icon.as_deref(), Some("\u{1F4B0}"));
    }

    /// The two commands that had never once worked: `aleph workspace create`
    /// sent `{"name": …}` at a handler that requires `id`, and
    /// `aleph workspace archive` did the same, so both returned
    /// `INVALID_PARAMS` on every invocation for as long as they had existed.
    ///
    /// This drives the handlers with the very types the CLI now constructs
    /// (`aleph_protocol::workspace::*`), so the request half of that gap cannot
    /// reopen without either failing here or failing to compile. The
    /// assertions are on the STORE, not on the response — a handler that
    /// answered `ok` while writing nothing would pass a response-only test.
    ///
    /// The last block re-asserts the historical shape is still rejected. That
    /// is what keeps this test honest: without it, a handler loosened to accept
    /// anything at all would look like a fix.
    #[tokio::test]
    async fn the_cli_create_and_archive_shapes_reach_their_handlers() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
                archive_after_days: 0,
            })
            .expect("agent env store"),
        );
        // `load_profiles` seeds "default" itself; this mirrors what
        // `start/mod.rs` does with an empty `[profiles]` section, which is the
        // shipped default and therefore the case that has to work.
        store.load_profiles(std::collections::HashMap::new());

        let created = handle_create(
            JsonRpcRequest::with_id(
                "workspace.create",
                Some(
                    serde_json::to_value(WorkspaceCreateParams {
                        id: "crypto".to_string(),
                        name: "Crypto Trading".to_string(),
                        description: Some("trading notes".to_string()),
                        icon: None,
                    })
                    .unwrap(),
                ),
                json!(1),
            ),
            store.clone(),
        )
        .await;
        assert!(
            created.is_success(),
            "the CLI's create shape must be accepted: {:?}",
            created.error
        );
        let row = store.get("crypto").await.unwrap().expect("row must exist");
        assert_eq!(row.name, "Crypto Trading");
        assert_eq!(row.description.as_deref(), Some("trading notes"));

        let archived = handle_archive(
            JsonRpcRequest::with_id(
                "workspace.archive",
                Some(
                    serde_json::to_value(WorkspaceRef {
                        id: "crypto".to_string(),
                    })
                    .unwrap(),
                ),
                json!(1),
            ),
            store.clone(),
        )
        .await;
        assert!(
            archived.is_success(),
            "the CLI's archive shape must be accepted: {:?}",
            archived.error
        );
        assert!(
            store.get("crypto").await.unwrap().is_none(),
            "archive must actually soft-delete the row"
        );

        // …and the shape that was broken is still a rejection, not a silently
        // accepted alias.
        for (method, params) in [
            ("workspace.create", json!({ "name": "crypto" })),
            ("workspace.archive", json!({ "name": "crypto" })),
        ] {
            let resp = if method == "workspace.create" {
                handle_create(
                    JsonRpcRequest::with_id(method, Some(params), json!(1)),
                    store.clone(),
                )
                .await
            } else {
                handle_archive(
                    JsonRpcRequest::with_id(method, Some(params), json!(1)),
                    store.clone(),
                )
                .await
            };
            assert_eq!(
                resp.error.as_ref().map(|e| e.code),
                Some(INVALID_PARAMS),
                "{method} must still require `id`"
            );
        }
    }

    /// Every column `aleph workspace list` prints has to exist in what this
    /// handler actually emits.
    ///
    /// It did not. The table read `status` and `created`; an `AgentEnv`
    /// serializes `is_archived` and `created_at`. Both columns were rendered
    /// with `.unwrap_or("-")`, so every row printed dashes and read as "this
    /// workspace has no status yet" rather than "this client is asking for
    /// fields that do not exist" — and neither side's tests could see it,
    /// because neither side ever looked at the other.
    ///
    /// The projection is asserted here, on the server, because this is the
    /// side that owns the field names.
    #[tokio::test]
    async fn every_column_the_cli_renders_is_present_in_the_list_response() {
        use aleph_protocol::workspace::WorkspaceList;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
                archive_after_days: 0,
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store
            .create("crypto", "default", Some("trading notes"))
            .await
            .unwrap();

        let resp = handle_list(
            JsonRpcRequest::with_id("workspace.list", None, json!(1)),
            store.clone(),
        )
        .await;
        let list: WorkspaceList = serde_json::from_value(resp.result.expect("result"))
            .expect("the CLI's list projection must parse the real response");

        let row = list
            .workspaces
            .iter()
            .find(|w| w.id == "crypto")
            .expect("the created workspace must be listed");
        assert_eq!(row.name, "crypto", "name defaults to the id server-side");
        assert_eq!(row.description.as_deref(), Some("trading notes"));
        assert!(
            row.created_at.timestamp() > 0,
            "created_at must be a real timestamp, not a default"
        );
    }

    #[test]
    fn test_set_agent_params_with_agent() {
        let json = serde_json::json!({"channel_id": "rpc", "agent_id": "project-x"});
        let params: SetAgentParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.channel_id, "rpc");
        assert_eq!(params.agent_id.as_deref(), Some("project-x"));
    }

    #[test]
    fn test_set_agent_params_unbind() {
        let json = serde_json::json!({"channel_id": "rpc"});
        let params: SetAgentParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.channel_id, "rpc");
        assert!(params.agent_id.is_none());
    }

    // ── channels.set_agent ghost-binding validation (AS-1) ────────────────
    // Mirrors the `agent_switch` tool's helpers (builtin_tools/agent_manage/switch.rs).

    use crate::gateway::agent_env::AgentEnvStoreConfig;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use tempfile::tempdir;

    fn test_workspace_mgr() -> Arc<AgentEnvStore> {
        let temp = tempdir().unwrap();
        let config = AgentEnvStoreConfig {
            db_path: temp.keep().join("test.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        Arc::new(AgentEnvStore::new(config).unwrap())
    }

    fn test_session_store() -> Arc<dyn crate::gateway::session_store::SessionStore> {
        let temp = tempdir().unwrap();
        let cfg = SessionManagerConfig {
            db_path: temp.keep().join("sessions.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(cfg).expect("session manager"))
    }

    fn test_instance(agent_id: &str) -> AgentInstance {
        let root = tempdir().unwrap().keep();
        let config = AgentInstanceConfig {
            agent_id: agent_id.to_string(),
            workspace: root.join("workspace"),
            agent_dir: root.join("state"),
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };
        AgentInstance::new(config, test_session_store()).expect("instance")
    }

    async fn registry_with(agent_id: &str) -> Arc<AgentRegistry> {
        let registry = Arc::new(AgentRegistry::new());
        registry.register(test_instance(agent_id)).await;
        registry
    }

    fn set_agent_req(channel: &str, agent: Option<&str>) -> JsonRpcRequest {
        let params = match agent {
            Some(a) => json!({"channel_id": channel, "agent_id": a}),
            None => json!({"channel_id": channel}),
        };
        JsonRpcRequest::with_id("channels.set_agent", Some(params), json!(1))
    }

    #[tokio::test]
    async fn set_agent_rejects_ghost_when_registry_present() {
        let wm = test_workspace_mgr();
        let registry = registry_with("trader").await;
        let resp = handle_set_agent(
            set_agent_req("telegram", Some("ghost")),
            Arc::clone(&wm),
            Some(registry),
            None,
        )
        .await;
        assert!(resp.is_error());
        let msg = resp.error.unwrap().message;
        assert!(msg.contains("not found"), "unexpected: {msg}");
        assert!(msg.contains("trader"), "should list available: {msg}");
        // The rejected bind persisted nothing.
        assert!(wm.get_active_agent("telegram").unwrap().is_none());
    }

    #[tokio::test]
    async fn set_agent_binds_existing_agent() {
        let wm = test_workspace_mgr();
        let registry = registry_with("trader").await;
        let resp = handle_set_agent(
            set_agent_req("telegram", Some("trader")),
            Arc::clone(&wm),
            Some(registry),
            None,
        )
        .await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        assert_eq!(
            wm.get_active_agent("telegram").unwrap().as_deref(),
            Some("trader")
        );
    }

    #[tokio::test]
    async fn set_agent_skips_validation_without_registry() {
        // A minimal server with no runtime registry must not block binds
        // (graceful fallback — the prior unchecked behavior).
        let wm = test_workspace_mgr();
        let resp = handle_set_agent(
            set_agent_req("telegram", Some("ghost")),
            Arc::clone(&wm),
            None,
            None,
        )
        .await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        assert_eq!(
            wm.get_active_agent("telegram").unwrap().as_deref(),
            Some("ghost")
        );
    }

    #[tokio::test]
    async fn set_agent_unbind_needs_no_registry() {
        let wm = test_workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        // Unbind (agent_id: None) never consults the registry.
        let resp =
            handle_set_agent(set_agent_req("telegram", None), Arc::clone(&wm), None, None).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        assert!(wm.get_active_agent("telegram").unwrap().is_none());
        // The unbind reports which agent was displaced.
        let result = resp.result.unwrap();
        assert_eq!(result["previous_agent"], json!("trader"));
    }

    #[tokio::test]
    async fn set_agent_reports_previous_and_no_op() {
        let wm = test_workspace_mgr();
        let registry = registry_with("trader").await;

        let first = handle_set_agent(
            set_agent_req("telegram", Some("trader")),
            Arc::clone(&wm),
            Some(Arc::clone(&registry)),
            None,
        )
        .await;
        let first = first.result.unwrap();
        assert_eq!(first["previous_agent"], serde_json::Value::Null);
        assert_eq!(first["no_op"], json!(false));

        // Re-binding to the same agent is a reported no-op, not an error.
        let second = handle_set_agent(
            set_agent_req("telegram", Some("trader")),
            Arc::clone(&wm),
            Some(registry),
            None,
        )
        .await;
        let second = second.result.unwrap();
        assert_eq!(second["previous_agent"], json!("trader"));
        assert_eq!(second["no_op"], json!(true));
    }

    // ── agents.bindings many-to-one shape ─────────────────────────────────

    #[tokio::test]
    async fn agent_bindings_reports_all_channels_per_agent() {
        let wm = test_workspace_mgr();
        // Many-to-one: two channels bound to the same agent must BOTH appear
        // (the old one-channel-per-agent map collapsed them to one).
        wm.set_active_agent("telegram", "trader").unwrap();
        wm.set_active_agent("discord", "trader").unwrap();

        let req = JsonRpcRequest::with_id("agents.bindings", None, json!(1));
        let resp = handle_agent_bindings(req, Arc::clone(&wm)).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(
            result["bindings"]["trader"],
            json!(["discord", "telegram"]),
            "all bound channels should be listed, sorted"
        );
    }
}
