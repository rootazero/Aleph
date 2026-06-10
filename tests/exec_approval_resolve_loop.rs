//! 集成测试：审批决策侧闭环 —— ManagerCallbackSink 收到按钮回调 →
//! ExecApprovalManager.resolve → 唤醒 wait_for_decision 的阻塞侧。
//!
//! 投递侧（adapter → bridge → Telegram capability）依赖真实通道，
//! 由 `/e2e-verify` 手动验证；此处覆盖与通道无关的 resolve 半环。

use std::sync::Arc;
use std::time::Duration;

use alephcore::approval::callback_sink::ManagerCallbackSink;
use alephcore::exec::{
    ApprovalDecisionType, ApprovalRequest, CommandAnalysis, ExecApprovalManager,
};
use alephcore::gateway::inbound_router::approval_callback::ApprovalCallbackSink;

fn request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        id: id.to_string(),
        command: "code_exec".to_string(),
        cwd: None,
        analysis: CommandAnalysis::error("danger-tier"),
        agent_id: "main".to_string(),
        session_key: "telegram:123456".to_string(),
        reason: None,
    }
}

#[tokio::test]
async fn approve_callback_wakes_blocked_waiter() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-approve"), 5_000);
    let id = record.id.clone();

    let m2 = manager.clone();
    let waiter = tokio::spawn(async move { m2.wait_for_decision(record).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sink = ManagerCallbackSink::new(manager.clone());
    let out = sink
        .handle_callback(&format!("approve:{}:once", id), "user-1")
        .await
        .expect("approval callback");
    assert!(out.resolved, "pending approval must resolve");

    let decision = waiter.await.unwrap();
    assert_eq!(decision, Some(ApprovalDecisionType::AllowOnce));
}

#[tokio::test]
async fn deny_callback_wakes_blocked_waiter() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-deny"), 5_000);
    let id = record.id.clone();

    let m2 = manager.clone();
    let waiter = tokio::spawn(async move { m2.wait_for_decision(record).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sink = ManagerCallbackSink::new(manager.clone());
    sink.handle_callback(&format!("approve:{}:deny", id), "user-1")
        .await
        .expect("approval callback");

    assert_eq!(waiter.await.unwrap(), Some(ApprovalDecisionType::Deny));
}

#[tokio::test]
async fn timeout_when_no_callback_arrives() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-timeout"), 100); // 100ms 超时
                                                               // 不发回调 → wait_for_decision 应在超时后返回 None。
    assert_eq!(manager.wait_for_decision(record).await, None);
}

#[tokio::test]
async fn unknown_callback_does_not_resolve() {
    let manager = Arc::new(ExecApprovalManager::new());
    let sink = ManagerCallbackSink::new(manager);
    let out = sink
        .handle_callback("approve:ghost-id:once", "user-1")
        .await
        .expect("is an approval callback");
    assert!(!out.resolved);
}
