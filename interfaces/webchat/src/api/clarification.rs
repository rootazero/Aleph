use crate::context::DashboardState;
use crate::state::notifications::PendingAskView;
use serde::Deserialize;

// ============================================================================
// Clarification API (`ask_user` question card)
// ============================================================================

pub struct ClarificationApi;

#[derive(Deserialize)]
struct PendingListResp {
    pending: Vec<PendingItem>,
}

#[derive(Deserialize)]
struct PendingItem {
    /// Clarification registry key — what `clarification.resolve` is called with.
    session_key: String,
    question: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Deserialize)]
struct ResolveResp {
    resolved: bool,
}

impl ClarificationApi {
    /// List the questions core is currently parked on.
    ///
    /// The `stream.ask_user` frame is a one-shot push, so a panel that connects
    /// (or reloads) mid-question would otherwise never learn a tool is blocked
    /// on its answer. Same role as `exec.approvals.pending` for approvals.
    pub async fn list_pending(state: &DashboardState) -> Result<Vec<PendingAskView>, String> {
        let result = state
            .rpc_call("clarification.pending", serde_json::Value::Null)
            .await?;
        let resp: PendingListResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse pending clarifications: {e}"))?;
        Ok(resp
            .pending
            .into_iter()
            .map(|p| PendingAskView {
                session_key: p.session_key,
                question: p.question,
                options: p.options,
            })
            .collect())
    }

    /// Answer the question `session_key` is parked on.
    ///
    /// `reply` is interpreted by core exactly as a channel reply is: a bare
    /// 1-based number picks that option, anything else is free text. The panel
    /// sends the index string for a choice click — the same payload Telegram's
    /// `clarify:<idx>` button produces — and never interprets it itself (R4).
    ///
    /// Returns whether a waiter was actually unblocked. `false` = the question
    /// was already gone (expired / superseded / run cancelled), so the reply
    /// answered nobody — the caller must NOT treat the text as consumed.
    pub async fn resolve(
        state: &DashboardState,
        session_key: &str,
        reply: &str,
    ) -> Result<bool, String> {
        let params = serde_json::json!({
            "session_key": session_key,
            "reply": reply,
        });
        let result = state.rpc_call("clarification.resolve", params).await?;
        let resp: ResolveResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse clarification.resolve: {e}"))?;
        Ok(resp.resolved)
    }
}
