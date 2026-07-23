//! Node-side approval requester (cluster ③).
//!
//! The node runs headless; its `ApprovalGate` would otherwise auto-deny every
//! capability escalation (`requester=None`). This requester instead routes the
//! prompt UP to the center over the now-bidirectional reverse-RPC channel and
//! maps the center's decision back to an `ApprovalOutcome`. Fail-closed: a
//! missing channel (disconnected), a transport error, or a timeout all map to
//! `Denied` — never a silent auto-approve.
//!
//! Redlines: pure routing, no LLM reasoning (R7); not in `src/harness/` (R10).

use crate::sync_primitives::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::json;

use crate::cluster::ReverseRpcChannel;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;

/// Shared, per-connection-refreshed channel slot. `run_session` writes
/// `Some(channel)` on connect and `None` on disconnect; the requester reads it
/// per call. `None` ⇒ fail-closed `Denied`.
pub type ApprovalSlot = Arc<RwLock<Option<ReverseRpcChannel>>>;

/// Node-side timeout for the reverse approval call. Deliberately ABOVE the
/// center's `DEFAULT_APPROVAL_TIMEOUT_MS` (120s) so the center decides first and
/// returns an explicit `"timeout"` outcome; this is only a transport-death
/// backstop.
pub const NODE_APPROVAL_TIMEOUT_MS: u64 = 130_000;

/// Map the center's outcome string back to an `ApprovalOutcome`. Any unknown
/// value (including `"denied"`) is fail-closed `Denied`.
pub(crate) fn outcome_from_str(s: &str) -> ApprovalOutcome {
    match s {
        "approved" => ApprovalOutcome::Approved,
        "approved_session" => ApprovalOutcome::ApprovedForSession,
        "timeout" => ApprovalOutcome::Timeout,
        _ => ApprovalOutcome::Denied,
    }
}

pub struct CenterApprovalRequester {
    slot: ApprovalSlot,
}

impl CenterApprovalRequester {
    pub const fn new(slot: ApprovalSlot) -> Self {
        Self { slot }
    }
}

#[async_trait]
impl ApprovalRequester for CenterApprovalRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        // Clone the channel out of the lock and drop the guard before awaiting —
        // a std RwLock guard is not Send.
        let channel = self.slot.read().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(channel) = channel else {
            tracing::warn!("node approval requested with no live center channel; denying");
            return ApprovalOutcome::Denied.into();
        };
        // `action` carries the redacted summary the center's operator card
        // renders — without it the operator approves a bare tool name.
        let params = json!({
            "tool": action.tool_name,
            "reason": action.reason,
            "action": action.summary,
        });
        match channel
            .call("node.approval.request", params, NODE_APPROVAL_TIMEOUT_MS)
            .await
        {
            Ok(resp) if resp.is_success() => {
                let result = resp.result.as_ref();
                let outcome = result
                    .and_then(|r| r.get("outcome"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("denied");
                // The operator's own words, when the center attached them to a
                // denial — optional field, absent from older centers.
                let deny_reason = result
                    .and_then(|r| r.get("deny_reason"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                ApprovalResponse {
                    outcome: outcome_from_str(outcome),
                    deny_reason,
                }
            }
            Ok(_) => ApprovalOutcome::Denied.into(),
            Err(e) => {
                tracing::warn!(error = %e, "node approval reverse-rpc failed; denying");
                ApprovalOutcome::Denied.into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;
    use serde_json::Value;
    use tokio::sync::mpsc;

    /// A `bash` escalation carrying its real command line.
    fn bash_action() -> ApprovalAction {
        ApprovalAction::for_tool_call(
            "bash",
            &json!({"cmd": "curl https://example.com"}),
            "needs network",
        )
    }

    fn slot_with_channel() -> (
        ApprovalSlot,
        mpsc::Receiver<String>,
        Arc<crate::cluster::PendingInvokes>,
    ) {
        let (out_tx, out_rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();
        let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
        (slot, out_rx, pending)
    }

    #[test]
    fn outcome_mapping_is_fail_closed() {
        assert_eq!(outcome_from_str("approved"), ApprovalOutcome::Approved);
        assert_eq!(
            outcome_from_str("approved_session"),
            ApprovalOutcome::ApprovedForSession
        );
        assert_eq!(outcome_from_str("timeout"), ApprovalOutcome::Timeout);
        assert_eq!(outcome_from_str("denied"), ApprovalOutcome::Denied);
        assert_eq!(outcome_from_str("garbage"), ApprovalOutcome::Denied);
    }

    #[tokio::test]
    async fn none_channel_denies() {
        let slot: ApprovalSlot = Arc::new(RwLock::new(None));
        let requester = CenterApprovalRequester::new(slot);
        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::Denied
        );
    }

    #[tokio::test]
    async fn round_trip_maps_center_outcome() {
        let (slot, mut out_rx, pending) = slot_with_channel();
        let requester = CenterApprovalRequester::new(slot);

        // Background "center": read the request frame, assert its shape, reply
        // with an approved_session outcome.
        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("request frame");
            let req: Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(req["method"], "node.approval.request");
            assert_eq!(req["params"]["tool"], "bash");
            assert_eq!(req["params"]["reason"], "needs network");
            // The center's operator card renders this — a bare tool name is an
            // operator deciding blind.
            assert_eq!(req["params"]["action"], "bash: curl https://example.com");
            let id = req["id"].clone();
            let resp =
                JsonRpcResponse::success(Some(id.clone()), json!({"outcome": "approved_session"}));
            pending.resolve(&id, resp);
        });

        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::ApprovedForSession
        );
    }

    #[tokio::test]
    async fn json_rpc_error_response_denies() {
        let (slot, mut out_rx, pending) = slot_with_channel();
        let requester = CenterApprovalRequester::new(slot);

        // Background "center": resolve the call with a JSON-RPC ERROR response
        // (`is_success()` is false) — the requester must fail-closed to Denied.
        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("request frame");
            let req: Value = serde_json::from_str(&frame).unwrap();
            let id = req["id"].clone();
            let resp = JsonRpcResponse::error(Some(id.clone()), -32000, "boom".to_string());
            pending.resolve(&id, resp);
        });

        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::Denied
        );
    }

    #[tokio::test]
    async fn transport_closed_denies() {
        let (out_tx, out_rx) = mpsc::channel::<String>(8);
        drop(out_rx); // closed transport → channel.call returns TransportClosed
        let channel = ReverseRpcChannel::new(out_tx);
        let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
        let requester = CenterApprovalRequester::new(slot);
        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::Denied
        );
    }
}
