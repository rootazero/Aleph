//! `projects.*` Gateway RPC client.
//!
//! Backs the desktop Panel's "enter project workspace ▾" picker — list / add an
//! existing folder / create a blank one / forget. The server-side store
//! lives at `crate::projects` in `alephcore`; this module just shapes the
//! JSON wire calls.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

/// Mirrors `gateway::handlers::projects::ProjectView`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
    pub last_used_at: i64,
}

pub struct ProjectsApi;

impl ProjectsApi {
    /// List recent projects, most-recently-used first.
    pub async fn list(state: &DashboardState) -> Result<Vec<ProjectInfo>, String> {
        let result = state
            .rpc_call("projects.list", serde_json::json!({}))
            .await?;
        let arr = result
            .get("projects")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(arr).map_err(|e| e.to_string())
    }

    /// Register an existing folder. `name` is optional — defaults to the
    /// folder's basename when omitted.
    pub async fn add(
        state: &DashboardState,
        path: &str,
        name: Option<&str>,
    ) -> Result<ProjectInfo, String> {
        let params = serde_json::json!({ "path": path, "name": name });
        let result = state.rpc_call("projects.add", params).await?;
        let project = result
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project in response".to_string())?;
        serde_json::from_value(project).map_err(|e| e.to_string())
    }

    /// Create a fresh empty directory under `parent` and register it.
    /// The directory must not already exist — the daemon rejects clashes
    /// so the user is never silently merged into someone else's folder.
    pub async fn create_blank(
        state: &DashboardState,
        parent: &str,
        name: &str,
    ) -> Result<ProjectInfo, String> {
        let params = serde_json::json!({ "parent": parent, "name": name });
        let result = state.rpc_call("projects.create_blank", params).await?;
        let project = result
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project in response".to_string())?;
        serde_json::from_value(project).map_err(|e| e.to_string())
    }

    /// Drop a project from the catalogue. The on-disk folder is untouched.
    pub async fn remove(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("projects.remove", params).await?;
        Ok(())
    }

    /// Bump `last_used_at` for a project (called when the user re-selects).
    pub async fn touch(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("projects.touch", params).await?;
        Ok(())
    }
}
