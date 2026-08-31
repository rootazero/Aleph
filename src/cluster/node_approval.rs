//! Node-side approval requester (cluster ③).
//!
//! The node runs headless; its `ApprovalGate` would otherwise auto-deny every
//! capability escalation (`requester=None`). This requester instead routes the
//! prompt UP to the center over the now-bidirectional reverse-RPC channel and
//! maps the center's decision back to an `ApprovalOutcome`.
//!
//! Fail-closed, and *how* it fails closed is the point: a missing channel, a
//! transport error, an error reply and a reply this node cannot parse all map
//! to [`ApprovalOutcome::Unavailable`] — **nobody was asked** — never to
//! `Denied`, and never to a silent auto-approve. `is_approved()` is false for
//! both, so the posture is identical; what differs is what the rest of the
//! system is told happened.
//!
//! This doc used to say those cases map to `Denied`, and called it deliberate.
//! It predates `ApprovalOutcome::Unavailable`. `Denied` is the word for
//! "a person refused": [`DenialLedger`](crate::sandbox::exec_approval::denial_ledger::DenialLedger)
//! makes it sticky for the action for the rest of the session, advances the
//! brute-force breaker (three of them pause every elevation gate on the node
//! for 300s and purge the tool-result store), and the model is handed
//! "The user already declined this exact action this session" — a sentence it
//! relays to a user who was never shown a card. A node whose center is
//! restarting produced all of that from a reconnect backoff. The node's ledger
//! key is one process-wide `SessionKey::ephemeral("node-<name>")`, so the
//! stickiness never expired either.
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
/// per call. `None` ⇒ fail-closed `Unavailable` (nobody could be asked).
pub type ApprovalSlot = Arc<RwLock<Option<ReverseRpcChannel>>>;

/// Node-side timeout for the reverse approval call. Deliberately ABOVE the
/// center's `DEFAULT_APPROVAL_TIMEOUT_MS` (120s) so the center decides first and
/// returns an explicit `"timeout"` outcome; this is only a transport-death
/// backstop.
pub(crate) const NODE_APPROVAL_TIMEOUT_MS: u64 = 130_000;

/// Map the center's outcome string back to an `ApprovalOutcome`.
///
/// `"denied"` is mapped explicitly so the consumer contract with the center is
/// in one place — and so the UNKNOWN arm can mean something else. An outcome
/// string this node does not recognise is center-side protocol drift, not a
/// person's refusal: nobody at the center said no, this node simply cannot read
/// the answer. It therefore falls closed to `Unavailable`, which is refused
/// exactly as hard but is not filed against the user.
pub(crate) fn outcome_from_str(s: &str) -> ApprovalOutcome {
    match s {
        "approved" => ApprovalOutcome::Approved,
        "approved_session" => ApprovalOutcome::ApprovedForSession,
        "timeout" => ApprovalOutcome::Timeout,
        "unavailable" => ApprovalOutcome::Unavailable,
        "denied" => ApprovalOutcome::Denied,
        // (B5-01) Drift guard: if the center ever adds a new outcome string
        // (e.g. `ApprovedWithConstraints`) and forgets to update this consumer,
        // this arm catches it. Warn so an operator can see the drift, and
        // classify it as `Unavailable` so it is refused without being recorded
        // as something the user did.
        //
        // Rate-limit the warning using the same pattern as B5-02 — a node whose
        // center drifts (or sends malformed outcomes in a flood) would otherwise
        // emit one warn! per call, swamping the log. Process-wide counter is
        // sufficient because a node process holds at most one `ApprovalSlot`.
        other => {
            use std::sync::atomic::{AtomicU64, Ordering};
            static DRIFT_COUNTER: AtomicU64 = AtomicU64::new(0);
            const WARN_EVERY: u64 = 100;
            let n = DRIFT_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_multiple_of(WARN_EVERY) {
                tracing::warn!(
                    outcome = %other,
                    count = n,
                    "node approval got an outcome string it does not recognize; \
                     fail-closed to Unavailable (drift, not a refusal)"
                );
            } else {
                tracing::debug!(
                    outcome = %other,
                    "node approval got an outcome string it does not recognize (drift)"
                );
            }
            ApprovalOutcome::Unavailable
        }
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
            // (B5-02) Rate-limit the no-channel warning: a headless node that
            // has lost its center connection would otherwise log the same line
            // for every escalation. Warn every 100th denial with the running
            // count; debug! for the rest. The counter is process-wide because
            // a node process holds at most one `ApprovalSlot`.
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            const WARN_EVERY: u64 = 100;
            let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_multiple_of(WARN_EVERY) {
                tracing::warn!(
                    count = n,
                    "node approval requested with no live center channel; refusing as unavailable"
                );
            } else {
                tracing::debug!("node approval unavailable: no live center channel");
            }
            return ApprovalOutcome::Unavailable.into();
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
                // A success reply with no `outcome` field is a center this
                // node cannot read, not a decision it made — same reasoning as
                // `outcome_from_str`'s unknown arm, and the sentinel routes
                // through it so there is one answer.
                let outcome = result
                    .and_then(|r| r.get("outcome"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unavailable");
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
            // A JSON-RPC error reply means the center could not run the
            // prompt, not that anyone answered it.
            Ok(_) => ApprovalOutcome::Unavailable.into(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "node approval reverse-rpc failed; refusing as unavailable"
                );
                ApprovalOutcome::Unavailable.into()
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
        // Drift is not a refusal: only the center saying "denied" is.
        assert_eq!(outcome_from_str("garbage"), ApprovalOutcome::Unavailable);
        assert!(!outcome_from_str("garbage").is_approved());
    }

    /// Every refusal this node MINTS ITSELF must be a non-decision.
    ///
    /// Formulated as a property rather than a list of expected variants, so a
    /// future arm that reintroduces a locally-minted `Denied` goes red without
    /// this test having to enumerate outcome names. The predicate is the
    /// ledger's own — `DenialReason::for_refusal(..).is_a_human_decision()` —
    /// which is what actually decides whether the refusal becomes sticky, feeds
    /// the brute-force breaker, and is described to the model as something the
    /// user did.
    #[tokio::test]
    async fn no_locally_minted_refusal_is_attributed_to_a_person() {
        use crate::sandbox::exec_approval::denial_ledger::DenialReason;

        let mut outcomes: Vec<(&str, ApprovalOutcome)> = Vec::new();

        // 1. No live channel.
        let requester = CenterApprovalRequester::new(Arc::new(RwLock::new(None)));
        outcomes.push((
            "no live center channel",
            requester.request_approval(&bash_action()).await.outcome,
        ));

        // 2. Closed transport.
        let (out_tx, out_rx) = mpsc::channel::<String>(8);
        drop(out_rx);
        let requester = CenterApprovalRequester::new(Arc::new(RwLock::new(Some(
            ReverseRpcChannel::new(out_tx),
        ))));
        outcomes.push((
            "closed transport",
            requester.request_approval(&bash_action()).await.outcome,
        ));

        // 3. JSON-RPC error reply, 4. success with no outcome field,
        // 5. an outcome string this node does not know.
        for (label, reply) in [
            ("json-rpc error reply", None),
            ("success without an outcome field", Some(json!({}))),
            (
                "unrecognized outcome string",
                Some(json!({"outcome": "approved_with_constraints"})),
            ),
        ] {
            let (slot, mut out_rx, pending) = slot_with_channel();
            let requester = CenterApprovalRequester::new(slot);
            tokio::spawn(async move {
                let frame = out_rx.recv().await.expect("request frame");
                let req: Value = serde_json::from_str(&frame).unwrap();
                let id = req["id"].clone();
                let resp = match reply {
                    Some(body) => JsonRpcResponse::success(Some(id.clone()), body),
                    None => JsonRpcResponse::error(Some(id.clone()), -32000, "boom".to_string()),
                };
                pending.resolve(&id, resp);
            });
            outcomes.push((
                label,
                requester.request_approval(&bash_action()).await.outcome,
            ));
        }

        for (label, outcome) in outcomes {
            assert!(!outcome.is_approved(), "{label} must still fail closed");
            let reason = DenialReason::for_refusal(outcome)
                .unwrap_or_else(|| panic!("{label} produced an approved outcome"));
            assert!(
                !reason.is_a_human_decision(),
                "{label} was filed as a human decision ({reason:?}) — it becomes sticky for the \
                 session, advances the brute-force breaker, and the model tells the user they \
                 declined something they were never shown"
            );
        }
    }

    #[tokio::test]
    async fn none_channel_is_unavailable_not_denied() {
        let slot: ApprovalSlot = Arc::new(RwLock::new(None));
        let requester = CenterApprovalRequester::new(slot);
        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::Unavailable
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
    async fn json_rpc_error_response_is_unavailable() {
        let (slot, mut out_rx, pending) = slot_with_channel();
        let requester = CenterApprovalRequester::new(slot);

        // Background "center": resolve the call with a JSON-RPC ERROR response
        // (`is_success()` is false) — the requester must fail closed, and to
        // `Unavailable`: the center could not run the prompt, so nobody
        // answered it.
        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("request frame");
            let req: Value = serde_json::from_str(&frame).unwrap();
            let id = req["id"].clone();
            let resp = JsonRpcResponse::error(Some(id.clone()), -32000, "boom".to_string());
            pending.resolve(&id, resp);
        });

        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::Unavailable
        );
    }

    #[tokio::test]
    async fn transport_closed_is_unavailable() {
        let (out_tx, out_rx) = mpsc::channel::<String>(8);
        drop(out_rx); // closed transport → channel.call returns TransportClosed
        let channel = ReverseRpcChannel::new(out_tx);
        let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
        let requester = CenterApprovalRequester::new(slot);
        assert_eq!(
            requester.request_approval(&bash_action()).await.outcome,
            ApprovalOutcome::Unavailable
        );
    }
}
