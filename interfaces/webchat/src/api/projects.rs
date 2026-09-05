//! `projects.*` Gateway RPC client.
//!
//! Backs the desktop Panel's "enter project workspace ▾" picker — list / add an
//! existing folder / create a blank one / forget. The server-side store
//! lives at `crate::projects` in `alephcore`; this module just shapes the
//! JSON wire calls.

use aleph_protocol::projects::{
    BindingPeerKind, ChannelBindParams, ChannelBindResult, ChannelBindingRow, ChannelListParams,
    ChannelListResult, ChannelUnbindParams, ChannelUnbindResult,
};

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

/// The shared wire row, not a weakened local copy of it.
///
/// This was a struct doc'd "mirrors `gateway::handlers::projects::ProjectView`"
/// — and `ProjectView` is itself only `pub type ProjectView =
/// aleph_protocol::projects::ProjectRow`, so the sentence was a third
/// representation of a shape with one author. The mirror was also a strict
/// subset (no `updated_at`) that put `#[serde(default)]` on `owner_user_id`,
/// `workspace_path`, `status` and `member_ids`: a server-side rename would have
/// degraded this client to `""` / `[]` on precisely the fields the ownership
/// line and the roster list are made of, while the CLI, parsing the same
/// method into the same shared row, hard-errored. Same remedy the `users` twin
/// took (`api::users`): one alias, no second declaration, every existing
/// `ProjectInfo` reference in this crate untouched.
///
/// Rendering rule that does not belong to the wire: `workspace_path` is `None`
/// for a project room bound to no folder. The picker only ever shows bound
/// rows, so it renders those as empty — but the two states stay distinguishable
/// here on purpose (the pre-P2 wire spelled both `""`).
pub use aleph_protocol::projects::ProjectRow as ProjectInfo;

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

    /// The channel conversations this room is bound to.
    ///
    /// No row type is declared on this side. The wire row has exactly one
    /// author — [`ChannelBindingRow`] in `aleph_protocol` — because a
    /// field-by-field copy on the client is how `aleph providers list` ended
    /// up rendering two columns the server had never sent. A Panel-local
    /// redeclaration would also silently tolerate the server dropping a field:
    /// parsing proves the client's fields are a subset of what arrived, never
    /// that the two are the same set.
    ///
    /// Open to a room's members server-side, unlike bind/unbind.
    pub async fn channel_list(
        state: &DashboardState,
        project_id: &str,
    ) -> Result<Vec<ChannelBindingRow>, String> {
        let params = to_params(&ChannelListParams {
            project_id: project_id.to_string(),
        })?;
        let result = state.rpc_call("projects.channel.list", params).await?;
        let parsed: ChannelListResult =
            serde_json::from_value(result).map_err(|e| e.to_string())?;
        Ok(parsed.bindings)
    }

    /// Point a channel group conversation at this room. **Admin-gated
    /// server-side** — a member reaches this page and its buttons are live, so
    /// the refusal must be classified by `components::admin_refusal` at the
    /// call site rather than shown raw.
    ///
    /// Returns the whole [`ChannelBindResult`], not a `bool`: the receipt has
    /// to tell the user what happened to the conversation's existing
    /// transcript, and that answer is three-valued.
    pub async fn channel_bind(
        state: &DashboardState,
        project_id: &str,
        channel_id: &str,
        peer_kind: BindingPeerKind,
        peer_id: &str,
        label: Option<&str>,
    ) -> Result<ChannelBindResult, String> {
        let params = to_params(&ChannelBindParams {
            project_id: project_id.to_string(),
            channel_id: channel_id.to_string(),
            peer_kind,
            peer_id: peer_id.to_string(),
            label: label.map(str::to_string),
        })?;
        let result = state.rpc_call("projects.channel.bind", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Release a conversation. Admin-gated server-side, like `channel_bind`.
    ///
    /// `unbound: false` means nothing was bound — a state, not a failure, and
    /// the caller renders it as such.
    pub async fn channel_unbind(
        state: &DashboardState,
        channel_id: &str,
        peer_kind: BindingPeerKind,
        peer_id: &str,
    ) -> Result<ChannelUnbindResult, String> {
        let params = to_params(&ChannelUnbindParams {
            channel_id: channel_id.to_string(),
            peer_kind,
            peer_id: peer_id.to_string(),
        })?;
        let result = state.rpc_call("projects.channel.unbind", params).await?;
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

/// Serialize a contract params type into the `Value` `rpc_call` takes.
///
/// The request is BUILT from the contract type rather than hand-written as a
/// `json!({..})`, which is the direction that matters: a hand-written object
/// and the server's `#[derive(Deserialize)]` have no way to disagree until a
/// user reports the button has never worked. Three families shipped that
/// defect (`aleph workspace create`, the TUI's `agent.run`, `aleph providers
/// add`).
fn to_params<T: Serialize>(params: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(params).map_err(|e| e.to_string())
}

fn parse_member_ids(result: serde_json::Value) -> Result<Vec<String>, String> {
    let arr = result
        .get("member_ids")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    serde_json::from_value(arr).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared row must parse what the server actually sends.
    ///
    /// A missing `#[serde(default)]` on one optional makes the WHOLE document
    /// fail to deserialize — serde does not degrade field by field — and the
    /// symptom is "the section is empty", not "one field is missing". That is
    /// the same shape that made a single `{"source":"github"}` entry hide
    /// every plugin in a marketplace.
    ///
    /// This test lives on the Panel side even though the type is shared: it is
    /// the reconciliation, and a reconciliation belongs where the consumer is.
    #[test]
    fn a_row_without_optionals_still_parses() {
        let row: ChannelBindingRow = serde_json::from_str(
            r#"{"project_id":"p-1","channel_id":"telegram","peer_kind":"group",
                "peer_id":"c1","bound_at":0}"#,
        )
        .expect("optionals must be defaulted, not required");
        assert_eq!(row.label, None);
        assert_eq!(row.bound_by, None);
        assert_eq!(row.peer_kind, BindingPeerKind::Group);
    }

    /// An empty binding list is a list, not an error.
    ///
    /// A room with no conversations bound is the ordinary state, and this
    /// section's whole job is to offer the bind form in it. Reading that as a
    /// failure would put a red banner on a page that is working.
    #[test]
    fn a_room_with_no_bindings_parses_as_an_empty_list() {
        let parsed: ChannelListResult = serde_json::from_str(r#"{"bindings":[]}"#)
            .expect("an empty list is a state, not a failure");
        assert!(parsed.bindings.is_empty());
    }

    /// The server's row, whole. `projects.list` sends every field
    /// `render_project` builds, and the room header's ownership line and the
    /// settings roster are made of two of them.
    #[test]
    fn a_full_project_row_parses_with_its_owner_and_roster() {
        let row: ProjectInfo = serde_json::from_str(
            r#"{"id":"p-1","name":"Rowboat","owner_user_id":"u-1",
                "workspace_path":"/srv/rowboat","status":"active",
                "member_ids":["u-1","u-2"],"created_at":1,"updated_at":2,
                "last_used_at":3}"#,
        )
        .expect("the shared row must parse what render_project sends");
        assert_eq!(row.owner_user_id.as_deref(), Some("u-1"));
        assert_eq!(row.member_ids, vec!["u-1".to_string(), "u-2".to_string()]);
        assert_eq!(row.workspace_path.as_deref(), Some("/srv/rowboat"));
    }

    /// A server-side rename of the roster key is an ERROR here, not an empty
    /// roster.
    ///
    /// The local mirror this alias replaced carried `#[serde(default)]` on
    /// `owner_user_id` / `workspace_path` / `status` / `member_ids` — exactly
    /// the four fields the ownership line and the roster list read — so a
    /// renamed key parsed cleanly and the page rendered a room with no owner
    /// and no members. That is a claim, not an unknown. The CLI parses the
    /// same method into this same type and hard-errors; both clients now fail
    /// the same way, loudly, on the same day.
    #[test]
    fn a_renamed_roster_key_is_an_error_not_an_empty_roster() {
        let parsed = serde_json::from_str::<ProjectInfo>(
            r#"{"id":"p-1","name":"Rowboat","owner_user_id":"u-1",
                "workspace_path":"/srv/rowboat","status":"active",
                "memberIds":["u-1","u-2"],"created_at":1,"updated_at":2,
                "last_used_at":3}"#,
        );
        assert!(
            parsed.is_err(),
            "a roster key the server no longer sends must fail the parse, \
             not default to an empty roster"
        );
    }

    /// Requests are built from the contract type, so their keys are the
    /// server's keys by construction. This pins that the builder is actually
    /// used — `to_params` returning something else would compile fine.
    #[test]
    fn the_bind_request_is_the_contract_shape() {
        let v = to_params(&ChannelBindParams {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: BindingPeerKind::Thread,
            peer_id: "c1".into(),
            label: None,
        })
        .expect("params serialise");
        let keys: std::collections::BTreeSet<&str> = v
            .as_object()
            .expect("params are an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["channel_id", "peer_id", "peer_kind", "project_id"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "an omitted label must be omitted rather than sent as null"
        );
        assert_eq!(v["peer_kind"], serde_json::json!("thread"));
    }
}
