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
            analysis: CommandAnalysis::error("danger"),
            agent_id: "main".to_string(),
            session_key: "telegram:123".to_string(),
            reason: None,
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
        let id = record.id.clone();

        // 在后台任务里 wait（wait_for_decision 负责把 record 注册进 pending）
        let m2 = manager.clone();
        let waiter = tokio::spawn(async move { m2.wait_for_decision(record).await });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let sink = ManagerCallbackSink::new(manager.clone());
        let out = sink
            .handle_callback(&format!("approve:{}:once", id), "u1")
            .await
            .expect("is an approval callback");
        assert!(out.resolved);

        let decision = waiter.await.unwrap();
        assert_eq!(decision, Some(ApprovalDecisionType::AllowOnce));
    }
}
