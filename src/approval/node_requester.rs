//! `run_node_approval` — the center-side driver for a node-initiated approval
//! (cluster ③). Mirrors `OperatorApprovalRequester` but:
//!   - there is no turn-context (the node has no chat conversation),
//!   - node identity + the node's redacted ACTION SUMMARY + reason are encoded
//!     into the `ApprovalRequest` `command` field, so the existing Panel card
//!     (which renders
//!     `ExecApprovalRecord.command` after refetching `exec.approvals.pending`)
//!     shows the node context with ZERO frontend change,
//!   - it returns a wire outcome string instead of an `ApprovalOutcome`.
//!
//! Reuses the shared `ExecApprovalManager` so the operator's existing
//! `exec.approval.resolve` RPC wakes this driver's oneshot.

use crate::exec::analysis::CommandAnalysis;
use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

/// Map an `ExecApprovalManager` decision into the wire outcome string consumed
/// by the node's `outcome_from_str`. `AllowAlways` collapses to a session grant
/// (permanent device elevation is out of scope, same as Phase 2b).
const fn decision_to_wire(decision: Option<ApprovalDecisionType>) -> &'static str {
    match decision {
        Some(ApprovalDecisionType::AllowOnce) => "approved",
        Some(ApprovalDecisionType::AllowSession) => "approved_session",
        Some(ApprovalDecisionType::AllowAlways) => "approved_session",
        Some(ApprovalDecisionType::Deny) => "denied",
        None => "timeout",
    }
}

/// Drive one node-initiated approval to a decision. Blocks on the operator
/// decision (up to `DEFAULT_APPROVAL_TIMEOUT_MS`); callers MUST run this on a
/// spawned task so the connection's select loop is not blocked.
///
/// Returns the wire outcome string plus the operator's free-text denial
/// reason, if one was attached (`/deny <reason>` / RPC `reason`). The caller
/// puts it on the reverse-RPC response as an optional `"deny_reason"` field —
/// older nodes ignore unknown fields, newer ones relay it to their model.
pub async fn run_node_approval(
    manager: &ExecApprovalManager,
    event_bus: &GatewayEventBus,
    node_id: &str,
    node_name: &str,
    tool: &str,
    action: &str,
    reason: &str,
) -> (&'static str, Option<String>) {
    // `action` is the node's redacted action summary — what will actually run.
    // An older node sends none; fall back to the tool name rather than an empty
    // card.
    let shown = if action.is_empty() { tool } else { action };
    let command = format!("node '{node_name}': {shown} — {reason}");
    let request = ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        command,
        cwd: None,
        analysis: CommandAnalysis {
            ok: true,
            reason: None,
            segments: vec![],
            chains: None,
        },
        agent_id: format!("node:{node_id}"),
        session_key: String::new(),
        reason: (!reason.is_empty()).then(|| reason.to_string()),
        // Cluster-node approvals are resolved by the center operator (RPC path),
        // never a channel button, so the originator gate does not apply here.
        originator_user_id: None,
    };
    let record = manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
    // Register BEFORE publishing so an instantly-resolving operator cannot race
    // ahead of registration (mirrors operator_requester).
    let (approval_id, rx, timeout) = manager.register_pending(record);

    if let Err(e) = event_bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
        approval_id: approval_id.clone(),
        session_key: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        // A node approval belongs to no local tool row — it arrives over
        // reverse RPC, outside any tool dispatch.
        tool_call_id: None,
    }) {
        tracing::warn!(error = %e, "failed to publish ApprovalRequested for node approval");
    }

    let resolved = manager
        .await_registered(approval_id.clone(), rx, timeout)
        .await;
    let decision = resolved.decision;

    let frame = match decision {
        Some(d) => GatewayEventFrame::ApprovalResolved {
            approval_id,
            session_key: String::new(),
            decision: d,
            resolved_by: None,
        },
        None => GatewayEventFrame::ApprovalExpired {
            approval_id,
            session_key: String::new(),
        },
    };
    if let Err(e) = event_bus.publish_frame(&frame) {
        tracing::warn!(error = %e, "failed to publish final approval event for node approval");
    }

    (decision_to_wire(decision), resolved.deny_reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use std::time::Duration;

    #[test]
    fn decision_mapping() {
        assert_eq!(
            decision_to_wire(Some(ApprovalDecisionType::AllowOnce)),
            "approved"
        );
        assert_eq!(
            decision_to_wire(Some(ApprovalDecisionType::AllowSession)),
            "approved_session"
        );
        assert_eq!(
            decision_to_wire(Some(ApprovalDecisionType::AllowAlways)),
            "approved_session"
        );
        assert_eq!(decision_to_wire(Some(ApprovalDecisionType::Deny)), "denied");
        assert_eq!(decision_to_wire(None), "timeout");
    }

    #[tokio::test]
    async fn publishes_node_context_and_resolves() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let bus = event_bus.clone();
        let handle = tokio::spawn(async move {
            run_node_approval(
                &mgr,
                &bus,
                "node-1",
                "worker",
                "bash",
                "bash: curl https://example.com",
                "needs network",
            )
            .await
        });

        // Observe the ApprovalRequested frame, then resolve via the manager (as
        // the operator's exec.approval.resolve would).
        let mut approval_id: Option<String> = None;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            if let GatewayEventFrame::ApprovalRequested {
                approval_id: id, ..
            } = frame
            {
                approval_id = Some(id);
                break;
            }
        }
        let id = approval_id.expect("ApprovalRequested published");
        // The pending record carries the node-context command for the Panel card.
        // `list_pending() -> Vec<PendingApproval>` (manager.rs:381); each
        // `PendingApproval` exposes `.record.command` (same path the Panel's
        // exec.approvals.pending handler reads).
        let pending = manager.list_pending();
        assert!(
            pending.iter().any(|p| p.record.command
                == "node 'worker': bash: curl https://example.com — needs network"),
            "pending record must carry node-context command, got {:?}",
            pending
                .iter()
                .map(|p| &p.record.command)
                .collect::<Vec<_>>()
        );
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowSession, None));

        assert_eq!(handle.await.unwrap(), ("approved_session", None));
    }
}
