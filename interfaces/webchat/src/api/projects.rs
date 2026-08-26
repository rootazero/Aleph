//! `projects.*` Gateway RPC client.
//!
//! Backs the desktop Panel's "enter project workspace ▾" picker — list / add an
//! existing folder / create a blank one / forget. The server-side store
//! lives at `crate::projects` in `alephcore`; this module just shapes the
//! JSON wire calls.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

/// Mirrors `gateway::handlers::projects::ProjectView`.
///
/// `workspace_path` is `None` for a project room that is not bound to a folder
/// — the picker only ever shows bound rows, so it renders those as empty, but
/// the two states are distinguishable here on purpose (the pre-P2 wire spelled
/// both `""`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner_user_id: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub member_ids: Vec<String>,
    pub created_at: i64,
    pub last_used_at: i64,
}

/// One entry in a room workspace listing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceEntry {
    pub name: String,
    pub is_dir: bool,
    #[serde(default)]
    pub size: u64,
}

/// A room workspace listing: the entries, plus whether the room is bound at
/// all. `root_bound: false` is a state, not a failure — the tab renders a
/// bind prompt for it, which is why it cannot be flattened into an empty
/// `entries` list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceListing {
    #[serde(default)]
    pub root_bound: bool,
    #[serde(default)]
    pub entries: Vec<WorkspaceEntry>,
}

/// A text preview of one file under a room workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspacePreview {
    #[serde(default)]
    pub content: String,
    /// The server stopped at its byte cap. Rendered as a banner rather than
    /// silently, so a reader never mistakes a cut file for the whole one.
    #[serde(default)]
    pub truncated: bool,
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

    // `create_blank` and `remove` bindings lived here with zero callers.
    //
    // The Panel's three doors are `create` (a new unbound room), `add`
    // (register an existing folder) and `archive` (the non-destructive
    // forget) — nothing ever reached for either of these, and R10 says a
    // zero-consumer binding is withdrawn rather than kept for a future that
    // has to re-decide anyway. `projects.remove` in particular is now refused
    // outright for a room with a conversation (`ProjectStore::remove`), so a
    // client binding for it would have been a button that only ever errors.
    //
    // The server verbs still exist; if a "delete this catalogue entry" affordance
    // is ever designed, add the binding then, next to the UI that calls it.

    /// Bump `last_used_at` for a project (called when the user re-selects).
    pub async fn touch(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("projects.touch", params).await?;
        Ok(())
    }

    /// Create a fresh, unbound project room (no `workspace_path`). Not in the
    /// brief's 6-new-verb list, but the server handler (`projects.create`)
    /// already exists and is exercised by tests — without this binding the
    /// project-rooms list would have no way to ever gain a first row from
    /// the Panel. Binding a workspace afterwards is a separate, owner-level
    /// step (`bind_workspace`).
    pub async fn create(state: &DashboardState, name: &str) -> Result<ProjectInfo, String> {
        let params = serde_json::json!({ "name": name });
        let result = state.rpc_call("projects.create", params).await?;
        let project = result
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project in response".to_string())?;
        serde_json::from_value(project).map_err(|e| e.to_string())
    }

    /// Fetch a single project room by id. Errors (including "not found" /
    /// "not on the roster" — the server deliberately renders both alike, see
    /// `gateway::handlers::projects`) surface as `Err(String)`.
    pub async fn get(state: &DashboardState, id: &str) -> Result<ProjectInfo, String> {
        let result = state
            .rpc_call("projects.get", serde_json::json!({ "id": id }))
            .await?;
        let project = result
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project in response".to_string())?;
        serde_json::from_value(project).map_err(|e| e.to_string())
    }

    /// Get-or-create the room's canonical chat session key.
    ///
    /// A room is ONE conversation for every member (spec §6.4), so the mapping
    /// is server-side: this used to be a per-browser `localStorage` entry,
    /// which meant the second member to open a room found nothing, started a
    /// fresh session and never saw anyone else's turns. `agent_id` is only a
    /// proposal — a room whose session another member already claimed answers
    /// with that key regardless of which agent this Panel defaults to.
    pub async fn room_session(
        state: &DashboardState,
        id: &str,
        agent_id: &str,
    ) -> Result<String, String> {
        let params = serde_json::json!({ "id": id, "agent_id": agent_id });
        let result = state.rpc_call("projects.room_session", params).await?;
        result
            .get("session_key")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "missing session_key in response".to_string())
    }

    /// Rename a room. Owner-only server-side.
    pub async fn rename(
        state: &DashboardState,
        id: &str,
        name: &str,
    ) -> Result<ProjectInfo, String> {
        let params = serde_json::json!({ "id": id, "name": name });
        let result = state.rpc_call("projects.rename", params).await?;
        let project = result
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project in response".to_string())?;
        serde_json::from_value(project).map_err(|e| e.to_string())
    }

    /// Archive a room (not deletion — roster and memory survive). Owner-only
    /// server-side. Returns the room's id and its new status string.
    pub async fn archive(state: &DashboardState, id: &str) -> Result<(String, String), String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("projects.archive", params).await?;
        let id = result
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let status = result
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((id, status))
    }

    /// Bind (or, with `path: None`, unbind) the room's working directory.
    /// Owner-only; naming a path additionally requires config-tier or
    /// loopback authorization — the server's denial string is surfaced as-is
    /// rather than pre-judged client-side (see `require_directory_choice` in
    /// `gateway::handlers::projects`). Unbinding (`path: None`) is a
    /// de-escalation and is never gated that way.
    pub async fn bind_workspace(
        state: &DashboardState,
        id: &str,
        path: Option<&str>,
    ) -> Result<ProjectInfo, String> {
        let params = serde_json::json!({ "id": id, "path": path });
        let result = state.rpc_call("projects.bind_workspace", params).await?;
        let project = result
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project in response".to_string())?;
        serde_json::from_value(project).map_err(|e| e.to_string())
    }

    /// Add a member to the roster. Owner-only server-side. Returns the
    /// resulting member id list (the mutation's response IS the roster, so
    /// callers never need a follow-up `member.list` round trip).
    pub async fn member_add(
        state: &DashboardState,
        id: &str,
        user_id: &str,
    ) -> Result<Vec<String>, String> {
        let params = serde_json::json!({ "id": id, "user_id": user_id });
        let result = state.rpc_call("projects.member.add", params).await?;
        parse_member_ids(result)
    }

    /// Remove a member from the roster. Owner-only server-side; the owner
    /// itself cannot be removed (the server rejects that with
    /// `INVALID_PARAMS`). Returns the resulting member id list.
    pub async fn member_remove(
        state: &DashboardState,
        id: &str,
        user_id: &str,
    ) -> Result<Vec<String>, String> {
        let params = serde_json::json!({ "id": id, "user_id": user_id });
        let result = state.rpc_call("projects.member.remove", params).await?;
        parse_member_ids(result)
    }
    /// List one directory under the room's bound workspace. `rel_path` is
    /// relative to the bound root; `None` means the root itself.
    pub async fn workspace_list(
        state: &DashboardState,
        project_id: &str,
        rel_path: Option<&str>,
    ) -> Result<WorkspaceListing, String> {
        let mut params = serde_json::Map::new();
        params.insert(
            "project_id".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
        if let Some(rel) = rel_path {
            params.insert(
                "rel_path".to_string(),
                serde_json::Value::String(rel.to_string()),
            );
        }
        let result = state
            .rpc_call("projects.workspace.list", serde_json::Value::Object(params))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Preview one text file under the room's bound workspace.
    pub async fn workspace_read(
        state: &DashboardState,
        project_id: &str,
        rel_path: &str,
    ) -> Result<WorkspacePreview, String> {
        let result = state
            .rpc_call(
                "projects.workspace.read",
                serde_json::json!({ "project_id": project_id, "rel_path": rel_path }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}

fn parse_member_ids(result: serde_json::Value) -> Result<Vec<String>, String> {
    let arr = result
        .get("member_ids")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    serde_json::from_value(arr).map_err(|e| e.to_string())
}
