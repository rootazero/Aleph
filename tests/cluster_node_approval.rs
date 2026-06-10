//! Cluster ③ integration: node-initiated approval round trip, in-process.
//!
//! node CenterApprovalRequester --(node.approval.request frame)--> center loop
//!   -> run_node_approval -> ExecApprovalManager -> mock operator resolve
//!   -> JSON-RPC response -> node PendingInvokes -> ApprovalOutcome
//!
//! No socket: the node's outbound mpsc is the center's inbound, and the node's
//! PendingInvokes is resolved with the center's response — exactly the frames
//! that would cross the WS.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use alephcore::cluster::{ApprovalSlot, CenterApprovalRequester, ReverseRpcChannel};
use alephcore::exec::manager::ExecApprovalManager;
use alephcore::exec::socket::ApprovalDecisionType;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::events::GatewayEventFrame;
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use serde_json::{json, Value};

/// Spawn a center that services exactly one node.approval.request frame,
/// resolving it with `decision` (or letting it expire if `None`).
fn spawn_center(
    mut out_rx: tokio::sync::mpsc::Receiver<String>,
    pending: Arc<alephcore::cluster::PendingInvokes>,
    manager: Arc<ExecApprovalManager>,
    event_bus: Arc<GatewayEventBus>,
    decision: Option<ApprovalDecisionType>,
) {
    tokio::spawn(async move {
        let frame = out_rx.recv().await.expect("node sent a request frame");
        let req: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(req["method"], "node.approval.request");
        let id = req["id"].clone();
        let tool = req["params"]["tool"].as_str().unwrap().to_string();
        let reason = req["params"]["reason"].as_str().unwrap().to_string();

        // Mock operator: resolve the pending record as soon as it is published.
        // Subscribe BEFORE `run_node_approval` publishes the frame — a broadcast
        // receiver only sees messages sent after it subscribes, so subscribing
        // inside the spawned task would race the publish below and miss it.
        let mgr = manager.clone();
        let mut rx = event_bus.subscribe_typed();
        tokio::spawn(async move {
            for _ in 0..10 {
                let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
                else {
                    break;
                };
                if let GatewayEventFrame::ApprovalRequested { approval_id, .. } = f {
                    if let Some(d) = decision {
                        mgr.resolve(&approval_id, d, Some("operator".to_string()));
                    }
                    break;
                }
            }
        });

        let outcome = alephcore::approval::run_node_approval(
            &manager, &event_bus, "node-1", "worker", &tool, &reason,
        )
        .await;
        let resp = JsonRpcResponse::success(Some(id.clone()), json!({ "outcome": outcome }));
        pending.resolve(&id, resp);
    });
}

async fn run_case(decision: Option<ApprovalDecisionType>, expect: ApprovalOutcome) {
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(8);
    let channel = ReverseRpcChannel::new(out_tx);
    let pending = channel.pending();
    let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
    let requester = CenterApprovalRequester::new(slot);

    let manager = Arc::new(ExecApprovalManager::new());
    let event_bus = Arc::new(GatewayEventBus::new());
    spawn_center(out_rx, pending, manager, event_bus, decision);

    let outcome = requester.request_approval("bash", "needs network").await;
    assert_eq!(outcome, expect);
}

#[tokio::test]
async fn approve_once_round_trip() {
    run_case(
        Some(ApprovalDecisionType::AllowOnce),
        ApprovalOutcome::Approved,
    )
    .await;
}

#[tokio::test]
async fn approve_session_round_trip() {
    run_case(
        Some(ApprovalDecisionType::AllowSession),
        ApprovalOutcome::ApprovedForSession,
    )
    .await;
}

#[tokio::test]
async fn deny_round_trip() {
    run_case(Some(ApprovalDecisionType::Deny), ApprovalOutcome::Denied).await;
}

#[tokio::test(start_paused = true)]
async fn timeout_round_trip() {
    // decision = None: the mock operator never resolves, so the center's
    // ExecApprovalManager hits DEFAULT_APPROVAL_TIMEOUT_MS (120s) and replies
    // "timeout"; the node maps that to ApprovalOutcome::Timeout. start_paused
    // auto-advances virtual time when all tasks park, so this completes
    // near-instantly (NOT 120s of real wall-clock).
    run_case(None, ApprovalOutcome::Timeout).await;
}
