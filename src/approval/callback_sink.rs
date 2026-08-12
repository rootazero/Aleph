//! `ApprovalCallbackSink` implementation — delivers channel-button callbacks
//! into `ExecApprovalManager`.

use async_trait::async_trait;

use crate::exec::bridge::ApprovalBridge;
use crate::exec::manager::ExecApprovalManager;
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
        // Failed parse means this is not an approval callback — return None so
        // the router lets the request through.
        let (id, decision) = ApprovalBridge::parse_callback(callback_data)?;

        // Originator gate: a channel approval button may only be used by the
        // person whose message triggered the approval. In a group chat several
        // paired members see the same inline buttons; without this any of them
        // could approve (or deny) another member's action — the group-chat
        // approval bypass. `record_originator` returns `Some` only for a live
        // record that recorded an originator, so non-channel / legacy records
        // (`None`) fall through unchanged. Operators resolve via the
        // `exec.approval.resolve` RPC — a different path — so this channel-only
        // gate never blocks them.
        if let Some(originator) = self.manager.record_originator(&id) {
            if originator != user_id {
                return Some(ApprovalCallbackResult {
                    resolved: false,
                    response_text: "只有发起该操作的用户可以在此审批。".to_string(),
                });
            }
        }

        let resolved = self
            .manager
            .resolve(&id, decision, Some(user_id.to_string()));
        let response_text = if resolved {
            ApprovalBridge::decision_response_text(&decision).to_string()
        } else {
            "This approval has expired or already been processed.".to_string()
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
        let record = manager.create(&mock_request("rec-1"), 5_000);

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

    /// A record with no originator (non-channel / legacy) keeps the prior
    /// behaviour: any paired user may resolve — the gate is a no-op.
    #[tokio::test]
    async fn no_originator_record_is_resolvable_by_anyone() {
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
            out.resolved,
            "a no-originator record stays resolvable by anyone"
        );

        let _ = waiter.await.unwrap();
    }
}
