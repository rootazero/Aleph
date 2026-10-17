use crate::context::{DashboardState, RpcFailure};
use crate::state::telecom_approvals::{
    parse_pending, parse_review, resolve_params, TelecomApprovalReview, TelecomApprovalView,
};

// ============================================================================
// Telecom A/B Approval API (operator approval card)
// ============================================================================

/// Match the server ledger's hard cap so every live approval is enumerable.
const PENDING_LIMIT: usize = 256;

pub struct TelecomApprovalApi;

impl TelecomApprovalApi {
    /// List pending telecom approvals (operator-only; a non-operator gets
    /// `AUTH_REQUIRED`, which callers treat as an empty list).
    pub async fn list_pending(
        state: &DashboardState,
    ) -> Result<Vec<TelecomApprovalView>, RpcFailure> {
        let result = state
            .rpc_call_with_code(
                "telecom.approval.pending",
                serde_json::json!({ "limit": PENDING_LIMIT }),
            )
            .await?;
        parse_pending(result).map_err(|message| RpcFailure {
            code: None,
            message,
        })
    }

    /// Load the complete body of one pending approval.
    pub async fn review(state: &DashboardState, id: &str) -> Result<TelecomApprovalReview, String> {
        let result = state
            .rpc_call("telecom.approval.review", serde_json::json!({ "id": id }))
            .await?;
        parse_review(result, id)
    }

    /// Resolve a pending approval from the review returned by
    /// [`Self::review`]. The id, stage and complete body all come from that
    /// review — never the list preview; the server binds the decision to
    /// exactly that text.
    pub async fn resolve(
        state: &DashboardState,
        review: &TelecomApprovalReview,
        approve: bool,
    ) -> Result<(), String> {
        state
            .rpc_call("telecom.approval.resolve", resolve_params(review, approve))
            .await?;
        Ok(())
    }
}
