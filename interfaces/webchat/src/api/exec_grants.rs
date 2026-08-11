//! Standing approval grants — `exec.grants.list` / `exec.grant.revoke`.
//!
//! The read/revoke face of what "Allow for this session" and "Always allow"
//! actually created. Pure I/O (R4): the server decides what this caller may
//! see and what a revoke is allowed to touch; nothing here filters.

use crate::context::DashboardState;
use serde::Deserialize;

/// One standing grant, as the server renders it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GrantView {
    /// Opaque action fingerprint — the revoke key. Never shown in full.
    pub fingerprint: String,
    pub tool: String,
    /// The redacted one-liner the human read on the card when they allowed it.
    /// This is the only field that makes a row identifiable to a person.
    pub summary: String,
    /// `"session"` or `"always"`.
    pub scope: String,
    pub granted_at_ms: u64,
    #[serde(default)]
    pub granted_by: Option<String>,
    #[serde(default)]
    pub session_key: Option<String>,
}

#[derive(Deserialize)]
struct GrantListResp {
    grants: Vec<GrantView>,
}

pub struct ExecGrantsApi;

impl ExecGrantsApi {
    /// Every standing grant this caller may see, newest first.
    pub async fn list(state: &DashboardState) -> Result<Vec<GrantView>, String> {
        let result = state
            .rpc_call("exec.grants.list", serde_json::Value::Null)
            .await?;
        let resp: GrantListResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse standing approvals: {e}"))?;
        Ok(resp.grants)
    }

    /// Take one back. `scope` and `session_key` come from the row — the same
    /// action can hold a grant at both scopes, and the server refuses to guess
    /// which one was meant.
    pub async fn revoke(state: &DashboardState, grant: &GrantView) -> Result<(), String> {
        let mut params = serde_json::json!({
            "fingerprint": grant.fingerprint,
            "scope": grant.scope,
        });
        if let Some(key) = grant.session_key.as_ref() {
            params["session_key"] = serde_json::Value::String(key.clone());
        }
        state.rpc_call("exec.grant.revoke", params).await?;
        Ok(())
    }
}
