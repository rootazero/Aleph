//! `workspace.*` RPC surface — the agent-environment roster behind
//! `/settings/workspaces`.
//!
//! # Typed against the shared contract, not against hand-written structs
//!
//! Every shape here is [`aleph_protocol::workspace`], the same module the
//! server builds its responses **from**. That is deliberate and it is the
//! second time this family has needed it: the CLI once declared the wire shape
//! a second time and disagreed with the handler, so `aleph workspace create`
//! returned `INVALID_PARAMS` on every call it had ever made while both sides'
//! tests stayed green. A shared type makes a rename a compile error on every
//! client at once, which is the only guard that holds across a crate boundary.
//!
//! So: no `json!({ "id": … })` literals below, and no local mirror structs. If
//! a field is missing here, add it to the protocol crate.
//!
//! # Errors pass through verbatim
//!
//! [`crate::context::DashboardState::rpc_call`] surfaces the server's
//! `error.message`, and the whole `workspace.` family is admin-gated
//! (`gateway::method_admin`), so a member gets the operator-privilege refusal
//! from every call here. Nothing in this module interprets that — the caller
//! runs it through [`crate::components::admin_refusal`], which is what keeps a
//! refused read from being rendered as "you have no workspaces". A parse
//! failure gets its own framing because it is a different fact; a transport
//! failure gets none, because inventing copy for it would put a guess in front
//! of the cause.

use aleph_protocol::workspace::{
    WorkspaceCreateParams, WorkspaceDetail, WorkspaceEnvelope, WorkspaceList, WorkspaceListParams,
    WorkspaceRef, WorkspaceRow, WorkspaceUpdateParams,
};
use serde_json::Value;

use crate::context::DashboardState;

/// Workspace (agent environment) management calls.
pub struct WorkspaceApi;

/// Serialize params, tagging the method so a failure names itself.
///
/// These cannot fail for the contract types as they stand — every field is a
/// `String`/`bool`/`Option<String>` — but the alternative is `.unwrap()` in a
/// WASM binary, where a panic takes the whole Panel down rather than one page.
fn encode<T: serde::Serialize>(method: &str, params: &T) -> Result<Value, String> {
    serde_json::to_value(params).map_err(|e| format!("Failed to encode {method} params: {e}"))
}

/// Parse a `{"workspace": …}` envelope — `get`, `create`, `update`, `unarchive`
/// all answer in it.
///
/// One helper rather than four call sites, because the failure it reports is
/// "the server sent a shape this Panel cannot read", and that sentence should
/// not have four wordings.
fn envelope(method: &str, result: Value) -> Result<WorkspaceDetail, String> {
    serde_json::from_value::<WorkspaceEnvelope>(result)
        .map(|e| e.workspace)
        .map_err(|e| format!("Invalid {method} response: {e}"))
}

impl WorkspaceApi {
    /// List workspaces. `include_archived` widens the view; the default is
    /// active rows only.
    ///
    /// A missing `workspaces` key is an error rather than an empty list —
    /// [`WorkspaceList`] has no serde default for exactly this reason, and the
    /// distinction matters here more than anywhere: "you have no workspaces"
    /// and "this response is broken" render identically otherwise.
    pub async fn list(
        state: &DashboardState,
        include_archived: bool,
    ) -> Result<Vec<WorkspaceRow>, String> {
        let params = encode("workspace.list", &WorkspaceListParams { include_archived })?;
        let result = state.rpc_call("workspace.list", params).await?;
        serde_json::from_value::<WorkspaceList>(result)
            .map(|list| list.workspaces)
            .map_err(|e| format!("Invalid workspace.list response: {e}"))
    }

    /// Fetch one workspace by id. Reaches archived rows too — the server
    /// answers by exact id and reports `is_archived`.
    pub async fn get(state: &DashboardState, id: &str) -> Result<WorkspaceDetail, String> {
        let params = encode("workspace.get", &WorkspaceRef { id: id.to_string() })?;
        let result = state.rpc_call("workspace.get", params).await?;
        envelope("workspace.get", result)
    }

    /// Create a workspace.
    ///
    /// A collision comes back as the server's own refusal, passed through
    /// untouched — including the case where the id is held by an **archived**
    /// workspace, where that refusal names `unarchive`. Rewording it here would
    /// throw away the only part of the message the operator can act on.
    pub async fn create(
        state: &DashboardState,
        params: WorkspaceCreateParams,
    ) -> Result<WorkspaceDetail, String> {
        let encoded = encode("workspace.create", &params)?;
        let result = state.rpc_call("workspace.create", encoded).await?;
        envelope("workspace.create", result)
    }

    /// Patch a workspace's name, description or icon.
    ///
    /// Absent fields are left alone, not cleared. An archived workspace is
    /// refused by the server; [`Self::unarchive`] is the way back.
    pub async fn update(
        state: &DashboardState,
        params: WorkspaceUpdateParams,
    ) -> Result<WorkspaceDetail, String> {
        let encoded = encode("workspace.update", &params)?;
        let result = state.rpc_call("workspace.update", encoded).await?;
        envelope("workspace.update", result)
    }

    /// Archive (soft-delete) a workspace.
    ///
    /// Returns nothing on purpose: the server answers `{"ok": true}` because
    /// the row has just left the default view and there is nothing left to
    /// render. Callers refetch the list. [`Self::unarchive`] is the one that
    /// hands a row back, and it says why.
    pub async fn archive(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = encode("workspace.archive", &WorkspaceRef { id: id.to_string() })?;
        state.rpc_call("workspace.archive", params).await?;
        Ok(())
    }

    /// Restore an archived workspace, and return it.
    ///
    /// The restored row comes back in the response — a caller re-renders from
    /// it instead of racing a follow-up [`Self::get`].
    pub async fn unarchive(state: &DashboardState, id: &str) -> Result<WorkspaceDetail, String> {
        let params = encode("workspace.unarchive", &WorkspaceRef { id: id.to_string() })?;
        let result = state.rpc_call("workspace.unarchive", params).await?;
        envelope("workspace.unarchive", result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;

    /// A real `workspace.list` response parses, and a broken one does NOT come
    /// back as zero workspaces.
    ///
    /// The second half is the point. `workspace.` is admin-gated, so the empty
    /// state and the refused state are the two things a member could see here,
    /// and a page that renders "no workspaces yet" for either is the exact
    /// defect `components::admin_refusal` exists to prevent — one level below
    /// where that module can help, because by then the error is already gone.
    #[test]
    fn a_response_missing_the_key_is_an_error_not_an_empty_roster() {
        let good = serde_json::json!({
            "workspaces": [{
                "id": "crypto",
                "name": "Crypto Trading",
                "description": null,
                "created_at": "2026-08-09T09:00:00Z",
                "is_archived": false,
            }]
        });
        let parsed: WorkspaceList = serde_json::from_value(good).expect("a real response parses");
        assert_eq!(parsed.workspaces.len(), 1);
        assert_eq!(parsed.workspaces[0].id, "crypto");

        assert!(
            serde_json::from_value::<WorkspaceList>(serde_json::json!({})).is_err(),
            "a missing `workspaces` key must not read as an empty roster"
        );
    }

    /// The envelope helper reports a shape it cannot read, and does not
    /// disguise it as something else.
    ///
    /// Also pins that its framing keeps the admin refusal recognisable: the
    /// page hands whatever comes out of these calls to
    /// `admin_refusal::settings_load_error`, which matches by substring, so any
    /// wrapper that replaced the message instead of framing it would silently
    /// downgrade a permission verdict to a parse complaint.
    #[test]
    fn a_shape_this_panel_cannot_read_is_reported_as_such() {
        let err = envelope("workspace.get", serde_json::json!({ "nope": 1 }))
            .expect_err("a missing envelope key must not parse");
        assert!(err.contains("workspace.get"), "{err}");

        assert!(crate::components::admin_refusal::is_admin_refusal(
            &format!("Invalid workspace.get response: {ADMIN_REQUIRED_MESSAGE}")
        ));
    }
}
