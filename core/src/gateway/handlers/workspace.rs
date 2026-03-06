//! Workspace RPC Handlers
//!
//! Handlers for workspace management: create, list, get, update, archive, switch, getActive.
//! All handlers delegate to WorkspaceManager (SQLite-backed).

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use super::parse_params;
use crate::gateway::workspace::WorkspaceManager;
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
    workspace_manager: Arc<WorkspaceManager>,
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
            let _ = workspace_manager
                .update(
                    &params.id,
                    Some(&ws.name),
                    None,
                    params.icon.as_deref(),
                )
                .await;

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
            format!("Failed to create workspace: {}", e),
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
pub async fn handle_list(
    request: JsonRpcRequest,
    workspace_manager: Arc<WorkspaceManager>,
) -> JsonRpcResponse {
    match workspace_manager.list(false).await {
        Ok(workspaces) => {
            JsonRpcResponse::success(request.id, json!({ "workspaces": workspaces }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list workspaces: {}", e),
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
pub async fn handle_get(
    request: JsonRpcRequest,
    workspace_manager: Arc<WorkspaceManager>,
) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match workspace_manager.get(&params.id).await {
        Ok(Some(ws)) => JsonRpcResponse::success(request.id, json!({ "workspace": ws })),
        Ok(None) => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get workspace: {}", e),
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
    workspace_manager: Arc<WorkspaceManager>,
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
            format!("Failed to update workspace: {}", e),
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
    workspace_manager: Arc<WorkspaceManager>,
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
            format!("Failed to archive workspace: {}", e),
        ),
    }
}

// ============================================================================
// Switch
// ============================================================================

/// Parameters for workspace.switch
#[derive(Debug, Deserialize)]
pub struct SwitchParams {
    /// Agent (workspace) identifier to switch to
    pub agent_id: String,
    /// Channel identifier (e.g., "telegram", "rpc")
    #[serde(default = "default_channel")]
    pub channel: String,
    /// Peer identifier within the channel
    #[serde(default = "default_peer_id")]
    pub peer_id: String,
}

fn default_channel() -> String {
    "rpc".to_string()
}

fn default_peer_id() -> String {
    "owner".to_string()
}

/// Switch the active agent/workspace for a channel+peer
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.switch","params":{"agent_id":"project-x","channel":"rpc","peer_id":"owner"},"id":1}
/// ```
pub async fn handle_switch(
    request: JsonRpcRequest,
    workspace_manager: Arc<WorkspaceManager>,
) -> JsonRpcResponse {
    let params: SwitchParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Verify workspace exists
    match workspace_manager.get(&params.agent_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Workspace '{}' not found", params.agent_id),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get workspace: {}", e),
            );
        }
    }

    // Set active agent for the channel+peer
    if let Err(e) = workspace_manager.set_active_agent(&params.channel, &params.peer_id, &params.agent_id) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to switch workspace: {}", e),
        );
    }

    // Touch the workspace to update last_active_at
    let _ = workspace_manager.touch(&params.agent_id).await;

    JsonRpcResponse::success(
        request.id,
        json!({
            "ok": true,
            "agent_id": params.agent_id,
        }),
    )
}

// ============================================================================
// GetActive
// ============================================================================

/// Parameters for workspace.getActive
#[derive(Debug, Deserialize)]
pub struct GetActiveParams {
    /// Channel identifier (e.g., "telegram", "rpc")
    #[serde(default = "default_channel")]
    pub channel: String,
    /// Peer identifier within the channel
    #[serde(default = "default_peer_id")]
    pub peer_id: String,
}

/// Get the current active agent/workspace for a channel+peer
///
/// Returns the active agent_id or "main" as default.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.getActive","params":{"channel":"rpc","peer_id":"owner"},"id":1}
/// ```
pub async fn handle_get_active(
    request: JsonRpcRequest,
    workspace_manager: Arc<WorkspaceManager>,
) -> JsonRpcResponse {
    // Parse params — allow missing params (defaults applied)
    let (channel, peer_id) = match &request.params {
        Some(p) => {
            let params: GetActiveParams = match serde_json::from_value(p.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Invalid params: {}", e),
                    );
                }
            };
            (params.channel, params.peer_id)
        }
        None => (default_channel(), default_peer_id()),
    };

    let agent_id = workspace_manager
        .get_active_agent(&channel, &peer_id)
        .unwrap_or(None)
        .unwrap_or_else(|| "main".to_string());

    // Fetch workspace to get the profile name
    let profile = match workspace_manager.get(&agent_id).await {
        Ok(Some(ws)) => ws.profile,
        _ => "default".to_string(),
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "agent_id": agent_id,
            "profile": profile,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_switch_params_deserialization() {
        let json = serde_json::json!({"agent_id": "project-x"});
        let params: SwitchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.agent_id, "project-x");
        assert_eq!(params.channel, "rpc"); // default
        assert_eq!(params.peer_id, "owner"); // default
    }

    #[test]
    fn test_switch_params_with_channel() {
        let json = serde_json::json!({"agent_id": "project-x", "channel": "telegram", "peer_id": "user-123"});
        let params: SwitchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.agent_id, "project-x");
        assert_eq!(params.channel, "telegram");
        assert_eq!(params.peer_id, "user-123");
    }

    #[test]
    fn test_get_active_params_deserialization() {
        let json = serde_json::json!({});
        let params: GetActiveParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.channel, "rpc"); // default
        assert_eq!(params.peer_id, "owner"); // default
    }

    #[test]
    fn test_get_active_params_with_channel() {
        let json = serde_json::json!({"channel": "telegram", "peer_id": "bob"});
        let params: GetActiveParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.channel, "telegram");
        assert_eq!(params.peer_id, "bob");
    }
}
