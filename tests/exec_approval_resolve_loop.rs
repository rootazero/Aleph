//! 集成测试：审批决策侧闭环 —— ManagerCallbackSink 收到按钮回调 →
//! ExecApprovalManager.resolve → 唤醒 await_registered 的阻塞侧。
//!
//! 投递侧（adapter → bridge → Telegram capability）依赖真实通道，
//! 由 `/e2e-verify` 手动验证；此处覆盖与通道无关的 resolve 半环。

use std::sync::Arc;

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
        originator_user_id: None,
        grant_key: None,
        // The default ceiling every gate but the operator-tier confirm gate
        // raises its cards with.
        allowed_decisions: alephcore::exec::allowed_decisions::session_max(),
    }
}

#[tokio::test]
async fn approve_callback_wakes_blocked_waiter() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-approve"), 5_000);

    // register_pending 是同步的：返回即可被 resolve，回调不会抢在注册之前。
    let (id, rx, wait_timeout) = manager.register_pending(record);
    let m2 = manager.clone();
    let waiter = {
        let id = id.clone();
        tokio::spawn(async move { m2.await_registered(id, rx, wait_timeout).await })
    };

    let sink = ManagerCallbackSink::new(manager.clone());
    let out = sink
        .handle_callback(&format!("approve:{}:once", id), "user-1")
        .await
        .expect("approval callback");
    assert!(out.resolved, "pending approval must resolve");

    // `await_registered` returns a `ResolvedDecision { decision, deny_reason }`
    // since the round-4 `/deny <reason>` plumbing; an approval carries no reason.
    let resolved = waiter.await.unwrap();
    assert_eq!(resolved.decision, Some(ApprovalDecisionType::AllowOnce));
    assert!(resolved.deny_reason.is_none());
}

#[tokio::test]
async fn deny_callback_wakes_blocked_waiter() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-deny"), 5_000);

    let (id, rx, wait_timeout) = manager.register_pending(record);
    let m2 = manager.clone();
    let waiter = {
        let id = id.clone();
        tokio::spawn(async move { m2.await_registered(id, rx, wait_timeout).await })
    };

    let sink = ManagerCallbackSink::new(manager.clone());
    sink.handle_callback(&format!("approve:{}:deny", id), "user-1")
        .await
        .expect("approval callback");

    assert_eq!(
        waiter.await.unwrap().decision,
        Some(ApprovalDecisionType::Deny)
    );
}

#[tokio::test]
async fn timeout_when_no_callback_arrives() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-timeout"), 100); // 100ms 超时
                                                               // 不发回调 → await_registered 应在超时后返回 None。
    let (id, rx, wait_timeout) = manager.register_pending(record);
    assert_eq!(
        manager
            .await_registered(id, rx, wait_timeout)
            .await
            .decision,
        None
    );
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
