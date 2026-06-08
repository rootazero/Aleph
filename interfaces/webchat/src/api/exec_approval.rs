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
}

impl ExecApprovalApi {
    /// List pending operator approvals (the source of truth for the cards).
    pub async fn list_pending(state: &DashboardState) -> Result<Vec<PendingApprovalView>, String> {
        let result = state
            .rpc_call("exec.approvals.pending", serde_json::Value::Null)
            .await?;
        let resp: PendingListResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse pending approvals: {}", e))?;
        Ok(resp
            .pending
            .into_iter()
            .map(|p| PendingApprovalView {
                id: p.record.id,
                command: p.record.command,
                agent_id: p.record.agent_id,
                remaining_ms: p.remaining_ms,
            })
            .collect())
    }

    /// Resolve a pending approval. `decision` is the kebab-case wire value:
    /// "allow-once" | "allow-session" | "deny".
    pub async fn resolve(state: &DashboardState, id: String, decision: &str) -> Result<(), String> {
        let params = serde_json::json!({
            "id": id,
            "decision": decision,
            "resolved_by": "Operator (Panel)",
        });
        state.rpc_call("exec.approval.resolve", params).await?;
        Ok(())
    }
}
