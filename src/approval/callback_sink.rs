//! `ApprovalCallbackSink` implementation — delivers channel-button callbacks
//! into `ExecApprovalManager`.

use async_trait::async_trait;

use crate::exec::bridge::ApprovalBridge;
use crate::exec::manager::{ApprovalResolveOutcome, ExecApprovalManager};
use crate::gateway::inbound_router::approval_callback::{
    ApprovalCallbackResult, ApprovalCallbackSink,
};
use crate::sync_primitives::Arc;

/// Wraps `Arc<ExecApprovalManager>`, parses callbacks and resolves the
/// corresponding pending approval.
pub struct ManagerCallbackSink {
    manager: Arc<ExecApprovalManager>,
}

impl ManagerCallbackSink {
    #[must_use]
    pub const fn new(manager: Arc<ExecApprovalManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ApprovalCallbackSink for ManagerCallbackSink {
    async fn handle_callback(
        &self,
        callback_data: &str,
        user_id: &str,
    ) -> Option<ApprovalCallbackResult> {
        // Inspect the id BEFORE parsing the action: a known operator card is
        // never a channel capability, even for its originator or a user named
        // operator. None/missing originator below must not grant this access.
        if let Some(rest) = callback_data.strip_prefix("approve:") {
            if let Some((id, _action)) = rest.split_once(':') {
                if self.manager.is_operator_approval(id) {
                    return Some(ApprovalCallbackResult {
                        resolved: false,
                        response_text: "此审批仅可由操作员通过 operator RPC 处理。".to_string(),
                    });
                }
            }
        }
        // Failed parse means this is not an approval callback — return None so
        // the router lets the request through.
        let (id, decision) = ApprovalBridge::parse_callback(callback_data)?;

        // The ORIGINATOR gate and the resolution happen in ONE locked step
        // inside the manager (see `ExecApprovalManager::resolve_for_channel`):
        // a channel approval button may only be used by the person whose
        // message triggered the approval, and the card that is CHECKED must be
        // the card that is RESOLVED. Checking the originator against a
        // separate snapshot left a window in which the id — reusable once its
        // card is resolved — was re-registered for another user's card: a
        // delayed callback then answered a card its author never saw.
        //
        // A card that recorded NO originator is refused as well. The
        // originator is what binds a card to the person allowed to answer it;
        // without one the card would be answerable by ANY paired member — an
        // unattended run's approval answered by whoever happens to be in the
        // chat. Such a card is the operator's to answer, through the operator
        // RPC (`exec.approval.resolve`), which is a different path and stays
        // open. A card that is already resolved or expired has no live record
        // and is reported as expired.
        let outcome = self.manager.resolve_for_channel(&id, decision, user_id);
        let (resolved, response_text) = match outcome {
            ApprovalResolveOutcome::Resolved { decision } => (
                true,
                // The decision reported here is the EFFECTIVE one (post
                // clamp), so the reply states what was actually approved
                // instead of echoing the request. APPROVAL-R4-02.
                ApprovalBridge::decision_response_text(&decision).to_string(),
            ),
            ApprovalResolveOutcome::WrongOriginator => (
                false,
                "只有发起该操作的用户可以在此审批。".to_string(),
            ),
            ApprovalResolveOutcome::NoOriginator => (
                false,
                "此审批没有关联的发起人，只能由操作员通过 operator RPC 处理。".to_string(),
            ),
            ApprovalResolveOutcome::OperatorOnly => (
                false,
                "此审批仅可由操作员通过 operator RPC 处理。".to_string(),
            ),
            ApprovalResolveOutcome::NotFound
            | ApprovalResolveOutcome::NotLive
            | ApprovalResolveOutcome::NotOffered => (
                false,
                "This approval has expired or already been processed.".to_string(),
            ),
        };
        Some(ApprovalCallbackResult {
            resolved,
            response_text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::analysis::CommandAnalysis;
    use crate::exec::decision::ApprovalRequest;
    use crate::exec::socket::ApprovalDecisionType;

    fn mock_request(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            command: "code_exec".to_string(),
            cwd: None,
            // Not `error(..)`: this is a tool card, so there is no command
            // line — `CommandAnalysis::error` would trip the `debug_assert!`
            // in `ExecApprovalManager::create` (a card the human can only
            // ever deny should not be raised). `not_a_command()` is the
            // exact fixture: it carries `ok: true` so the assertion passes,
            // and `Vec::new()` segments because there is no argv to parse.
            analysis: CommandAnalysis::not_a_command(),
            agent_id: "main".to_string(),
            session_key: "telegram:123".to_string(),
            reason: None,
            originator_user_id: None,
            grant_key: None,
            allowed_decisions: crate::exec::allowed_decisions::session_max(),
        }
    }

    // Known operator ids are not channel capabilities, even for the matching
    // originator (or an actor called operator) and even before action parsing.
    #[tokio::test]
    async fn both_operator_kinds_refuse_channel_callbacks_without_consuming_approval() {
        for independent in [false, true] {
            for originator in [Some("alice".to_string()), None] {
                let manager = Arc::new(ExecApprovalManager::new());
                let mut request = mock_request("operator-card");
                request.originator_user_id = originator;
                let mut record = manager.create(&request, 5_000);
                record.operator_only = true;
                let (id, mut rx, timeout, guard) = if independent {
                    let (id, rx, timeout, guard) = manager.register_operator_once(record).unwrap();
                    (id, rx, timeout, Some(guard))
                } else {
                    let (id, rx, timeout) = manager.register_pending(record);
                    (id, rx, timeout, None)
                };
                let sink = ManagerCallbackSink::new(manager.clone());
                for actor in ["alice", "operator"] {
                    for action in ["once", "session", "always", "deny", "unknown-action"] {
                        let response = sink
                            .handle_callback(&format!("approve:{id}:{action}"), actor)
                            .await
                            .expect("operator card is rejected before parsing the action");
                        assert!(!response.resolved);
                        assert!(manager.get_pending(&id).is_some());
                        assert!(matches!(
                            rx.try_recv(),
                            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                        ));
                    }
                }
                assert!(manager.resolve(
                    &id,
                    ApprovalDecisionType::AllowOnce,
                    Some("operator".to_string())
                ));
                let resolved = if let Some(ref guard) = guard {
                    guard.wait(rx, timeout).await
                } else {
                    manager.await_registered(id, rx, timeout).await
                };
                assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
            }
        }
    }

    #[tokio::test]
    async fn non_callback_data_returns_none() {
        let sink = ManagerCallbackSink::new(Arc::new(ExecApprovalManager::new()));
        assert!(sink.handle_callback("hello world", "u1").await.is_none());
    }

    #[tokio::test]
    async fn unknown_id_reports_not_resolved() {
        let sink = ManagerCallbackSink::new(Arc::new(ExecApprovalManager::new()));
        let out = sink
            .handle_callback("approve:no-such-id:once", "u1")
            .await
            .expect("is an approval callback");
        assert!(!out.resolved);
        assert!(out.response_text.contains("expired"));
    }

    #[tokio::test]
    async fn pending_approval_gets_resolved() {
        let manager = Arc::new(ExecApprovalManager::new());
        let mut req = mock_request("rec-1");
        req.originator_user_id = Some("u1".to_string());
        let record = manager.create(&req, 5_000);

        // register_pending is synchronous: the entry is resolvable the instant
        // it returns, so the callback below cannot race ahead of registration.
        let (id, rx, wait_timeout) = manager.register_pending(record);
        let m2 = manager.clone();
        let waiter = {
            let id = id.clone();
            tokio::spawn(async move { m2.await_registered(id, rx, wait_timeout).await })
        };

        let sink = ManagerCallbackSink::new(manager.clone());
        let out = sink
            .handle_callback(&format!("approve:{}:once", id), "u1")
            .await
            .expect("is an approval callback");
        assert!(out.resolved);

        let resolved = waiter.await.unwrap();
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }

    /// Originator gate: a record that recorded an originator may be resolved via
    /// a channel button ONLY by that user — a different paired member is refused
    /// (the group-chat approval-bypass fix), while the originator resolves it.
    #[tokio::test]
    async fn originator_gate_blocks_a_non_originator() {
        let manager = Arc::new(ExecApprovalManager::new());
        let mut req = mock_request("rec-orig");
        req.originator_user_id = Some("alice".to_string());
        let record = manager.create(&req, 5_000);

        let (id, rx, wait_timeout) = manager.register_pending(record);
        let m2 = manager.clone();
        let waiter = {
            let id = id.clone();
            tokio::spawn(async move { m2.await_registered(id, rx, wait_timeout).await })
        };

        let sink = ManagerCallbackSink::new(manager.clone());
        // Bob (not the originator) taps approve — refused, record stays pending.
        let out = sink
            .handle_callback(&format!("approve:{}:once", id), "bob")
            .await
            .expect("is an approval callback");
        assert!(
            !out.resolved,
            "a non-originator must not resolve the approval"
        );

        // Alice (the originator) taps approve — resolves normally.
        let out = sink
            .handle_callback(&format!("approve:{}:once", id), "alice")
            .await
            .expect("is an approval callback");
        assert!(out.resolved, "the originator resolves the approval");

        let resolved = waiter.await.unwrap();
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }

    /// An approval id is reusable once its card is resolved. A DELAYED
    /// callback must therefore not answer the NEXT card registered under that
    /// id: the originator check and the resolution share one lock, so the
    /// callback is always judged against the card that is actually resolved.
    #[tokio::test]
    async fn a_delayed_callback_does_not_answer_a_reused_approval_id() {
        let manager = Arc::new(ExecApprovalManager::new());
        let mut alice = mock_request("shared-id");
        alice.originator_user_id = Some("alice".to_string());
        let record = manager.create(&alice, 5_000);
        let (id, rx, wait_timeout) = manager.register_pending(record);

        // Alice answers her own card, which then leaves the live set.
        let sink = ManagerCallbackSink::new(manager.clone());
        let out = sink
            .handle_callback(&format!("approve:{id}:once"), "alice")
            .await
            .expect("is an approval callback");
        assert!(out.resolved);
        assert_eq!(
            manager
                .await_registered(id.clone(), rx, wait_timeout)
                .await
                .decision,
            Some(ApprovalDecisionType::AllowOnce)
        );

        // The id is free again: Bob's card is raised under it.
        let mut bob = mock_request("shared-id");
        bob.originator_user_id = Some("bob".to_string());
        let record = manager.create(&bob, 5_000);
        let (bob_id, bob_rx, bob_timeout) = manager.register_pending(record);
        assert_eq!(bob_id, id);

        // Alice's delayed callback must not answer BOB's card.
        let out = sink
            .handle_callback(&format!("approve:{id}:once"), "alice")
            .await
            .expect("is an approval callback");
        assert!(
            !out.resolved,
            "a delayed callback must not answer another user's card"
        );
        assert!(
            manager.get_pending(&id).is_some(),
            "Bob's card stays pending for Bob"
        );

        // Bob answers his own card.
        let out = sink
            .handle_callback(&format!("approve:{bob_id}:once"), "bob")
            .await
            .expect("is an approval callback");
        assert!(out.resolved);
        assert_eq!(
            manager
                .await_registered(bob_id, bob_rx, bob_timeout)
                .await
                .decision,
            Some(ApprovalDecisionType::AllowOnce)
        );
    }

    /// A record with NO originator is refused for a channel button.
    ///
    /// The originator is what binds a card to the person allowed to answer
    /// it. Without one the card was answerable by ANY paired member — an
    /// unattended run's approval answered by whoever happened to be in the
    /// chat. Such a card is the operator's to answer, through the operator
    /// RPC (`exec.approval.resolve`), which is a different path and stays
    /// open.
    #[tokio::test]
    async fn a_no_originator_record_is_refused_for_channel_callbacks() {
        let manager = Arc::new(ExecApprovalManager::new());
        // `mock_request` leaves `originator_user_id = None`.
        let record = manager.create(&mock_request("rec-legacy"), 5_000);

        let (id, rx, wait_timeout) = manager.register_pending(record);
        let m2 = manager.clone();
        let waiter = {
            let id = id.clone();
            tokio::spawn(async move { m2.await_registered(id, rx, wait_timeout).await })
        };

        let sink = ManagerCallbackSink::new(manager.clone());
        let out = sink
            .handle_callback(&format!("approve:{}:once", id), "anyone")
            .await
            .expect("is an approval callback");
        assert!(
            !out.resolved,
            "a card nobody owns must not be answerable by a paired member"
        );
        assert!(
            manager.get_pending(&id).is_some(),
            "the refused card stays pending for the operator"
        );

        // The operator path is a different one, and still resolves it.
        assert!(manager.resolve(
            &id,
            ApprovalDecisionType::AllowOnce,
            Some("operator".to_string())
        ));
        let resolved = waiter.await.unwrap();
        assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    }
}
