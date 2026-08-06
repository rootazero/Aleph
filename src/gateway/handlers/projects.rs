//! Project workspace handlers.
//!
//! Backed by [`crate::projects::ProjectStore`] — see that module for the
//! on-disk format and locking story. These handlers are pure I/O: they
//! validate params, delegate to the store, and shape JSON responses.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use super::parse_params;
use crate::projects::{Project, ProjectError, ProjectStore};
use crate::sync_primitives::Arc;

/// Serializable view of a project — `path` is stringified so JSON consumers
/// (Panel, CLI) never have to deal with platform-specific `PathBuf` wire
/// formats.
///
/// An unbound room (no `workspace_path`) renders as an empty `path`. That is
/// tolerable only because no RPC can create one yet: `ProjectStore::create` is
/// reachable in this commit solely through the path-registering entry points.
/// The roster-RPC task replaces this view with the full entity (owner, status,
/// members, nullable workspace) and the Panel is updated in the same change.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
    pub last_used_at: i64,
}

impl From<Project> for ProjectView {
    fn from(p: Project) -> Self {
        Self {
            id: p.id,
            name: p.name,
            path: p
                .workspace_path
                .map(|w| w.to_string_lossy().to_string())
                .unwrap_or_default(),
            created_at: p.created_at,
            last_used_at: p.last_used_at,
        }
    }
}

// ============================================================================
// projects.list
// ============================================================================

pub async fn handle_list(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    match store.list() {
        Ok(projects) => {
            let view: Vec<ProjectView> = projects.into_iter().map(Into::into).collect();
            JsonRpcResponse::success(request.id, json!({ "projects": view }))
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.add
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AddParams {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
}

pub async fn handle_add(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: AddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = PathBuf::from(&params.path);
    match store.add(&path, params.name) {
        Ok(project) => {
            JsonRpcResponse::success(request.id, json!({ "project": ProjectView::from(project) }))
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.create_blank
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateBlankParams {
    pub parent: String,
    pub name: String,
}

pub async fn handle_create_blank(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
) -> JsonRpcResponse {
    let params: CreateBlankParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let parent = PathBuf::from(&params.parent);
    match store.create_blank(&parent, &params.name) {
        Ok(project) => {
            JsonRpcResponse::success(request.id, json!({ "project": ProjectView::from(project) }))
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.remove
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RemoveParams {
    pub id: String,
}

pub async fn handle_remove(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: RemoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match store.remove(&params.id) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "id": params.id, "removed": true })),
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.touch
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct TouchParams {
    pub id: String,
}

pub async fn handle_touch(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: TouchParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match store.touch(&params.id) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "id": params.id })),
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.get
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub id: String,
}

pub async fn handle_get(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match store.get(&params.id) {
        Ok(Some(project)) => {
            JsonRpcResponse::success(request.id, json!({ "project": ProjectView::from(project) }))
        }
        Ok(None) => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("project not found: {}", params.id),
        ),
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn project_error_response(id: Option<serde_json::Value>, err: ProjectError) -> JsonRpcResponse {
    let (code, msg) = match &err {
        ProjectError::NotFound(_) => (RESOURCE_NOT_FOUND, err.to_string()),
        ProjectError::NotAbsolute(_)
        | ProjectError::NotDirectory(_)
        | ProjectError::InvalidName(_) => (INVALID_PARAMS, err.to_string()),
        ProjectError::AlreadyExists(_) => (INVALID_PARAMS, err.to_string()),
        _ => (INTERNAL_ERROR, err.to_string()),
    };
    JsonRpcResponse::error(id, code, msg)
}
