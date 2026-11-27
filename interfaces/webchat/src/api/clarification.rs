use aleph_protocol::clarification::{
    ClarificationActionParams, ClarificationActionResponse, PendingListResponse,
};
use leptos::prelude::Update;

use crate::context::DashboardState;

// ============================================================================
// Clarification API (`ask_user` question card)
// ============================================================================

pub struct ClarificationApi;

impl ClarificationApi {
    /// Refresh the parked questions from core.
    ///
    /// `clarification.pending` is the authoritative snapshot; a push frame is
    /// only a notification. The snapshot is guarded by the generation it was
    /// fetched at: a push that arrives while the request is in flight is
    /// newer and survives the answer of this call.
    ///
    /// The same call is the reconciliation path after a terminal frame that
    /// named no request — reading is always safe, so it is also the recovery
    /// path after a failed mutation.
    pub async fn refresh_pending(state: &DashboardState) -> Result<(), String> {
        let start = state
            .clarifications
            .try_update(|board| board.begin_snapshot())
            .ok_or_else(|| "Clarification board is no longer available".to_string())?;
        let result = state
            .rpc_call("clarification.pending", serde_json::Value::Null)
            .await?;
        let snapshot: PendingListResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse pending clarifications: {e}"))?;
        state.clarifications.update(|board| {
            board.apply_snapshot(&snapshot, start);
        });
        Ok(())
    }

    /// Send one guarded action to the live request.
    ///
    /// The action carries the identity of the request and of the question it
    /// addresses (`expected_request_id` / `expected_question_id`), so core can
    /// refuse an action that was meant for a superseded request instead of
    /// answering the wrong one.
    pub async fn resolve_action(
        state: &DashboardState,
        params: ClarificationActionParams,
    ) -> Result<ClarificationActionResponse, String> {
        let payload = serde_json::to_value(params)
            .map_err(|e| format!("Failed to encode clarification action: {e}"))?;
        let result = state.rpc_call("clarification.resolve", payload).await?;
        let response: ClarificationActionResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse clarification.resolve: {e}"))?;
        Ok(response)
    }
}
