//! Workspace RPC Handlers
//!
//! Handlers for workspace management: create, list, get, update, archive.
//! Channel agent binding: `channels.set_agent`, agents.bindings.
//! All handlers delegate to `AgentEnvStore` (SQLite-backed).

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

/// Parameters for workspace.create
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    /// Workspace identifier (URL-safe slug)
    pub id: String,
    /// Human-readable display name
    pub name: String,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
    /// Optional emoji or icon identifier
    #[serde(default)]
    pub icon: Option<String>,
}

/// Create a new workspace
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.create","params":{"id":"crypto","name":"Crypto Trading"},"id":1}
/// ```
pub async fn handle_create(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

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
/// That is defense in depth, NOT a closed boundary, and the distinction is
/// recorded here rather than implied by the presence of a check: a workspace
/// id is a user-chosen name (`"project-aleph"`), it encodes no owner, and the
/// `agent_envs` table has no owner column — so an ordinary workspace passes
/// this predicate for every caller and one member can still read another's
/// `env_vars`. Closing it needs an owner column plus a migration (the stamp
/// `SessionMetadata`/`GroupChatSession`/`LoopState` carry), which is a
/// schema change and a product decision, not a handler fix. See the P1
/// final-fix report.
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

/// Parameters for workspace.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    /// Workspace identifier
    pub id: String,
}

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
    let params: GetParams = match parse_params(&request) {
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

/// Parameters for workspace.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    /// Workspace identifier
    pub id: String,
    /// New name (optional)
    #[serde(default)]
    pub name: Option<String>,
    /// New description (optional)
    #[serde(default)]
    pub description: Option<String>,
    /// New icon (optional)
    #[serde(default)]
    pub icon: Option<String>,
}

/// Update workspace metadata
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.update","params":{"id":"crypto","name":"Crypto Research"},"id":1}
/// ```
pub async fn handle_update(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

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
        Ok(None) => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        ),
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
pub async fn handle_archive(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match workspace_manager.archive(&params.id).await {
        Ok(true) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Ok(false) => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        ),
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
    /// for everyone — see [`handle_list`]'s doc for why closing that needs a
    /// schema change.
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

    #[test]
    fn test_create_params_deserialization() {
        let json = serde_json::json!({"id": "crypto", "name": "Crypto Trading"});
        let params: CreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name, "Crypto Trading");
        assert!(params.description.is_none());
    }

    #[test]
    fn test_create_params_with_optional_fields() {
        let json = serde_json::json!({"id": "novel", "name": "Novel", "description": "My novel project", "icon": "\u{1F4D6}"});
        let params: CreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.description.as_deref(), Some("My novel project"));
        assert_eq!(params.icon.as_deref(), Some("\u{1F4D6}"));
    }

    #[test]
    fn test_get_params_deserialization() {
        let json = serde_json::json!({"id": "crypto"});
        let params: GetParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
    }

    #[test]
    fn test_update_params_deserialization() {
        let json = serde_json::json!({"id": "crypto", "name": "Crypto Research"});
        let params: UpdateParams = serde_json::from_value(json).unwrap();
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
        let params: UpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name.as_deref(), Some("Crypto Research"));
        assert_eq!(params.description.as_deref(), Some("Updated description"));
        assert_eq!(params.icon.as_deref(), Some("\u{1F4B0}"));
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
