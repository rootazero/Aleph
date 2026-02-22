//! Workspace RPC Handlers
//!
//! Handlers for workspace management: create, list, get, update, archive.

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::workspace::Workspace;
use crate::memory::workspace_store;

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
pub async fn handle_create(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: CreateParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id and name required".to_string(),
            );
        }
    };

    let mut ws = Workspace::new(params.id, params.name);
    if let Some(desc) = params.description {
        ws.description = Some(desc);
    }
    if let Some(icon) = params.icon {
        ws.icon = Some(icon);
    }

    match workspace_store::create_workspace(&db, &ws).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "workspace": ws,
            }),
        ),
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
pub async fn handle_list(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    match workspace_store::list_workspaces(&db).await {
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
pub async fn handle_get(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: GetParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    match workspace_store::get_workspace(&db, &params.id).await {
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
pub async fn handle_update(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: UpdateParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    // Get existing workspace
    let mut ws = match workspace_store::get_workspace(&db, &params.id).await {
        Ok(Some(ws)) => ws,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Workspace '{}' not found", params.id),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to get workspace: {}", e),
            );
        }
    };

    // Apply updates
    if let Some(name) = params.name {
        ws.name = name;
    }
    if let Some(description) = params.description {
        ws.description = Some(description);
    }
    if let Some(icon) = params.icon {
        ws.icon = Some(icon);
    }
    ws.updated_at = chrono::Utc::now().timestamp();

    // Delete old fact and create updated one
    if let Err(e) = db.delete_fact(&format!("ws-{}", params.id)).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete old workspace fact: {}", e),
        );
    }

    match workspace_store::create_workspace(&db, &ws).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "workspace": ws,
            }),
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
pub async fn handle_archive(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: GetParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: id required".to_string(),
            );
        }
    };

    match workspace_store::archive_workspace(&db, &params.id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to archive workspace: {}", e),
        ),
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
}
