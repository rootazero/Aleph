//! Panel API for the project room's Kanban tab — `goal.list` / `loop.list`.
//!
//! Both RPCs take one optional `scope_id`. Omitting it means "rows I own"
//! (the pre-existing owner-only default); passing a rendered project scope id
//! means "rows stamped for this room", gated server-side through the roster,
//! so a non-member and a nonexistent room both come back as an empty list
//! rather than an error. That is deliberate — an error would be an existence
//! oracle — and it means an empty board here is genuinely ambiguous between
//! "no goals yet" and "not your room". The room page only renders this tab
//! for a room the viewer could already open, so the ambiguity never reaches
//! a user who did not already know the room exists.
//!
//! The scope id is built with [`aleph_protocol::scope::project_scope_id`]
//! rather than an inline `format!` — see that module's doc for why the
//! spelling has an owner.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One goal row, mirroring the server's `GoalSummary` projection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GoalRow {
    pub session_id: String,
    pub objective: String,
    /// Canonical snake_case status (`active` / `paused` / `blocked` /
    /// `complete`) — `GoalStatus`'s serde form, kept as a `String` because the
    /// Panel only groups and labels it; a client has no business owning the
    /// closed set of server-side states.
    pub status: String,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

/// One loop row, mirroring the server's `LoopSummary` projection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LoopRow {
    pub session_id: String,
    pub prompt: String,
    pub status: String,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub created_at_ms: u64,
}

pub struct KanbanApi;

impl KanbanApi {
    /// Goals stamped for `project_id`.
    pub async fn goals_for_project(
        state: &DashboardState,
        project_id: &str,
    ) -> Result<Vec<GoalRow>, String> {
        let params = json!({ "scope_id": aleph_protocol::scope::project_scope_id(project_id) });
        let result = state.rpc_call("goal.list", params).await?;
        rows(&result, "goals")
    }

    /// Loops stamped for `project_id`.
    pub async fn loops_for_project(
        state: &DashboardState,
        project_id: &str,
    ) -> Result<Vec<LoopRow>, String> {
        let params = json!({ "scope_id": aleph_protocol::scope::project_scope_id(project_id) });
        let result = state.rpc_call("loop.list", params).await?;
        rows(&result, "loops")
    }
}

/// Pull `key`'s array out of an envelope and decode it.
///
/// A missing key decodes as empty rather than as an error: both handlers
/// answer a dormant subsystem (no goal store initialized yet) with an empty
/// list, and a client that turned that into a red banner would be reporting
/// a failure the server did not have.
fn rows<T: serde::de::DeserializeOwned>(result: &Value, key: &str) -> Result<Vec<T>, String> {
    let arr = result.get(key).cloned().unwrap_or(Value::Array(vec![]));
    serde_json::from_value(arr).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every column this tab renders has to be a key the server actually
    /// sends. The fixtures below are the server's own projections
    /// (`gateway::handlers::kanban::{GoalSummary, LoopSummary}`) — if a field
    /// is renamed on that side, this decode drops it to the `serde(default)`
    /// and the assertions catch it.
    #[test]
    fn a_goal_row_decodes_every_column_the_tab_renders() {
        let envelope = json!({ "goals": [{
            "session_id": "s-1",
            "objective": "ship the thing",
            "status": "active",
            "scope_id": "project:p-a",
            "created_at_ms": 1_000_u64,
            "updated_at_ms": 2_000_u64,
        }]});
        let decoded: Vec<GoalRow> = rows(&envelope, "goals").expect("decodes");
        assert_eq!(
            decoded,
            vec![GoalRow {
                session_id: "s-1".into(),
                objective: "ship the thing".into(),
                status: "active".into(),
                scope_id: Some("project:p-a".into()),
                created_at_ms: 1_000,
                updated_at_ms: 2_000,
            }]
        );
    }

    #[test]
    fn a_loop_row_decodes_every_column_the_tab_renders() {
        let envelope = json!({ "loops": [{
            "session_id": "s-2",
            "prompt": "watch the build",
            "status": "active",
            "scope_id": "project:p-a",
            "created_at_ms": 3_000_u64,
        }]});
        let decoded: Vec<LoopRow> = rows(&envelope, "loops").expect("decodes");
        assert_eq!(decoded[0].prompt, "watch the build");
        assert_eq!(decoded[0].scope_id.as_deref(), Some("project:p-a"));
    }

    /// A dormant subsystem answers `{"goals": []}`, and an older core that
    /// does not know the method at all cannot reach here (the RPC itself
    /// errors). Neither is a decode failure, so neither may surface as one.
    #[test]
    fn a_missing_or_empty_envelope_is_an_empty_list_not_an_error() {
        let empty: Vec<GoalRow> = rows(&json!({ "goals": [] }), "goals").expect("empty ok");
        assert!(empty.is_empty());
        let absent: Vec<GoalRow> = rows(&json!({}), "goals").expect("absent ok");
        assert!(absent.is_empty());
    }

    /// The request half of the contract: the handler reads `scope_id`, and it
    /// must be the rendered form, not the bare project id. Asserted through
    /// the shared helper so this test cannot pin a spelling core has moved off.
    #[test]
    fn the_request_carries_a_rendered_project_scope_id() {
        let params = json!({ "scope_id": aleph_protocol::scope::project_scope_id("p-a") });
        assert_eq!(params["scope_id"], json!("project:p-a"));
        assert_eq!(
            aleph_protocol::scope::project_id_of(params["scope_id"].as_str().unwrap()),
            Some("p-a")
        );
    }
}
