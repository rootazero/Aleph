use crate::context::DashboardState;
use crate::state::notifications::PendingApprovalView;
use serde::Deserialize;

// ============================================================================
// Exec Approval API (operator approval card)
// ============================================================================

pub struct ExecApprovalApi;

#[derive(Deserialize)]
struct PendingListResp {
    pending: Vec<PendingItem>,
}

#[derive(Deserialize)]
struct PendingItem {
    record: PendingRecord,
    remaining_ms: u64,
}

#[derive(Deserialize)]
struct PendingRecord {
    id: String,
    command: String,
    agent_id: String,
    /// Session the requesting turn belongs to — lets the chat timeline attach
    /// the approval to the tool row of the conversation that is waiting.
    #[serde(default)]
    session_key: String,
    /// Harness call id of the tool this approval belongs to — how the inline
    /// card finds its tool row. Absent for approvals raised outside a tool call.
    #[serde(default)]
    tool_call_id: Option<String>,
    /// Why the approval was requested. Server-supplied; without it the
    /// operator only ever sees a bare tool name.
    #[serde(default)]
    reason: Option<String>,
    /// Absolute deadline the server stamped on the record. `0` is the
    /// no-expiry sentinel (attended approvals wait forever, ruled 2026-08-28);
    /// `remaining_ms` is meaningless for those and must not be read.
    #[serde(default)]
    expires_at_ms: i64,
    /// Which decision tiers this card may offer, kebab-case. Absent from an
    /// older core, where the historical session ceiling is the right reading —
    /// `default_decisions` supplies it rather than an empty list, since an
    /// empty list would render a card with no buttons at all.
    #[serde(default = "default_decisions")]
    allowed_decisions: Vec<String>,
}

/// What a card offers when the server did not say — the pre-2026-08-11 button
/// set, and never `allow-always`: a missing field may narrow, never widen.
fn default_decisions() -> Vec<String> {
    vec![
        "allow-once".to_string(),
        "allow-session".to_string(),
        "deny".to_string(),
    ]
}

impl ExecApprovalApi {
    /// List pending operator approvals (the source of truth for the cards).
    ///
    /// `remaining_ms` is a server-side snapshot taken at fetch time, so it is
    /// converted here into an absolute `expires_at_ms` deadline. A render-time
    /// snapshot can only ever print a frozen "expires in Ns"; an absolute
    /// deadline lets the card count down against the shared 1s clock.
    pub async fn list_pending(state: &DashboardState) -> Result<Vec<PendingApprovalView>, String> {
        let result = state
            .rpc_call("exec.approvals.pending", serde_json::Value::Null)
            .await?;
        let resp: PendingListResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse pending approvals: {e}"))?;
        let now = crate::views::chat::timeline::now_millis();
        Ok(resp
            .pending
            .into_iter()
            .map(|p| PendingApprovalView {
                id: p.record.id,
                command: p.record.command,
                agent_id: p.record.agent_id,
                session_key: p.record.session_key,
                tool_call_id: p.record.tool_call_id,
                reason: p.record.reason,
                // `0` (no-expiry sentinel) passes through untouched; only a
                // real deadline is re-based from the server's remaining_ms
                // snapshot onto the local clock.
                expires_at_ms: if p.record.expires_at_ms == 0 {
                    0
                } else {
                    now + p.remaining_ms as i64
                },
                allowed_decisions: p.record.allowed_decisions,
            })
            .collect())
    }

    /// Resolve a pending approval. `decision` is the kebab-case wire value:
    /// "allow-once" | "allow-session" | "allow-always" | "deny" — one of the
    /// values the card carried in `allowed_decisions`. The server narrows
    /// anything wider than that card offered, so this is a rendering contract,
    /// not the enforcement point. `reason` is the operator's
    /// free-text objection on a deny — the server relays it verbatim to the
    /// model ("The user said: …") so it re-plans on the actual objection.
    /// Blank reasons are dropped; the field is omitted rather than sent empty.
    pub async fn resolve(
        state: &DashboardState,
        id: String,
        decision: &str,
        reason: Option<String>,
    ) -> Result<(), String> {
        let mut params = serde_json::json!({
            "id": id,
            "decision": decision,
            "resolved_by": "Operator (Panel)",
        });
        if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
            params["reason"] = serde_json::Value::String(reason);
        }
        state.rpc_call("exec.approval.resolve", params).await?;
        Ok(())
    }
}
