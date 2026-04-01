//! Workspace RPC Handlers
//!
//! Handlers for workspace management: create, list, get, update, archive.
//! Channel agent binding: channels.set_agent, agents.bindings.
//! All handlers delegate to AgentEnvStore (SQLite-backed).

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use super::parse_params;
use crate::gateway::agent_env::AgentEnvStore;
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
            let _ = workspace_manager
                .update(&params.id, Some(&ws.name), None, params.icon.as_deref())
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
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    match workspace_manager.list(false).await {
        Ok(workspaces) => JsonRpcResponse::success(request.id, json!({ "workspaces": workspaces })),
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
    workspace_manager: Arc<AgentEnvStore>,
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
            format!("Failed to archive workspace: {}", e),
        ),
    }
}

// ============================================================================
// Channel Agent Binding (1:1 model)
// ============================================================================

/// Parameters for channels.set_agent
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
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"channels.set_agent","params":{"channel_id":"rpc","agent_id":"project-x"},"id":1}
/// ```
pub async fn handle_set_agent(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: SetAgentParams =
        match serde_json::from_value(request.params.clone().unwrap_or_default()) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
        };

    match params.agent_id {
        Some(agent_id) => match workspace_manager.set_active_agent(&params.channel_id, &agent_id) {
            Ok(()) => JsonRpcResponse::success(request.id, json!({"ok": true})),
            Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
        },
        None => match workspace_manager.clear_active_agent(&params.channel_id) {
            Ok(()) => JsonRpcResponse::success(request.id, json!({"ok": true})),
            Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
        },
    }
}

/// Get all agent→channel bindings for the Panel.
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
    match workspace_manager.get_all_agent_bindings() {
        Ok(bindings) => JsonRpcResponse::success(request.id, json!({"bindings": bindings})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
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
}
