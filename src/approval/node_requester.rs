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
use crate::sandbox::exec_approval::gate::ApprovalOutcome;

/// Render an outcome as the wire string consumed by the node's
/// `outcome_from_str`. The decision → outcome step is the shared
/// [`ApprovalDecisionType::to_outcome_within`] mapping, and the set this path
/// names is [`session_max`](crate::exec::allowed_decisions::session_max):
/// permanent device elevation is out of scope (same as Phase 2b), and a remote
/// center answering "always" must not mint an install-wide grant on this side.
/// This fn only fixes the cluster wire vocabulary, which must not change.
///
/// `ApprovedAlways` is unreachable here for that reason, and is rendered as the
/// session grant rather than silently as "denied" — an approval must never be
/// turned into a refusal by a rendering table.
const fn outcome_to_wire(outcome: ApprovalOutcome) -> &'static str {
    match outcome {
        ApprovalOutcome::Approved => "approved",
        ApprovalOutcome::ApprovedForSession | ApprovalOutcome::ApprovedAlways => "approved_session",
        ApprovalOutcome::Denied => "denied",
        ApprovalOutcome::Timeout => "timeout",
        // A node asked its center and the center had nobody to ask. Distinct
        // from "denied" for the same reason it is distinct in-process: the
        // node's own ledger would otherwise make the intent sticky and advance
        // its brute-force breaker over a decision nobody made. A center running
        // an older build never sends this token, and a node running an older
        // build maps the unknown token to `Denied` — fail-closed, which is the
        // pre-existing behaviour.
        ApprovalOutcome::Unavailable => "unavailable",
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
    // Sanitize cross-trust-boundary RPC fields before they are formatted
    // into the operator-facing `command` string. A malicious node can
    // otherwise inject newlines / ANSI escapes / control characters that
    // (a) impersonate entries in the audit log and pending-approvals panel
    // and (b) corrupt terminal/UI rendering when an operator opens the
    // card.
    let node_name = sanitize_for_display(node_name, 64);
    let reason = sanitize_for_display(reason, 256);
    let shown = sanitize_for_display(shown, 256);
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
        // The node's redacted summary is not a canonical action identity — no
        // session-grant cascade for node approvals.
        grant_key: None,
        // A node approval can be answered once; it carries no local action
        // identity for either grant tier to key on.
        allowed_decisions: crate::exec::allowed_decisions::session_max(),
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
        // Mirrors operator_requester::request_approval (APPROVAL-R3-003):
        // publish failure is fatal — the center operator was never notified,
        // so a "waiting" card never appeared on their surface. Deny the
        // approval and remove the pending entry so the node's await does
        // NOT spin for the full DEFAULT_APPROVAL_TIMEOUT_MS against a
        // notification nobody will ever act on.
        tracing::warn!(error = %e, "failed to publish ApprovalRequested for node approval");
        manager.resolve(
            &approval_id,
            ApprovalDecisionType::Deny,
            Some("unavailable".to_string()),
        );
        return (
            "unavailable",
            Some(
                "approval notification could not be delivered to the operator surface".to_string(),
            ),
        );
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

    let outcome = decision.map_or(ApprovalOutcome::Timeout, |d| {
        d.to_outcome_within(&crate::exec::allowed_decisions::session_max())
    });
    (outcome_to_wire(outcome), resolved.deny_reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::socket::ApprovalDecisionType;
    use crate::sync_primitives::Arc;
    use std::time::Duration;

    #[test]
    fn decision_mapping() {
        // The wire strings are the cluster protocol and must not change; the
        // decision → outcome step is the shared `to_outcome` mapping.
        let wire = |d: Option<ApprovalDecisionType>| {
            outcome_to_wire(d.map_or(ApprovalOutcome::Timeout, |d| {
                d.to_outcome_within(&crate::exec::allowed_decisions::session_max())
            }))
        };
        assert_eq!(wire(Some(ApprovalDecisionType::AllowOnce)), "approved");
        assert_eq!(
            wire(Some(ApprovalDecisionType::AllowSession)),
            "approved_session"
        );
        assert_eq!(
            wire(Some(ApprovalDecisionType::AllowAlways)),
            "approved_session"
        );
        assert_eq!(wire(Some(ApprovalDecisionType::Deny)), "denied");
        assert_eq!(wire(None), "timeout");
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

/// Sanitize a remote-RPC string before it is embedded in the operator-facing
/// `command` field or rendered into a UI card.
///
/// Strips control characters (incl. newline, which would let a malicious
/// node forge a fake audit-log entry) and truncates to `max_len` bytes.
/// ANSI escape sequences are stripped to defend terminal/UI rendering.
fn sanitize_for_display(s: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_len));
    for c in s.chars() {
        if c.is_control() {
            out.push(' ');
        } else if c == '\u{1b}' {
            // ESC — drop the start of any ANSI sequence by replacing with space.
            out.push(' ');
        } else {
            out.push(c);
        }
        if out.len() >= max_len {
            out.push('…');
            break;
        }
    }
    out
}
