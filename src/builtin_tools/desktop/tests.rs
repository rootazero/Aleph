//! Tests for the desktop tool.

use super::*;
use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use async_trait::async_trait;

fn make_args(action: &str) -> DesktopArgs {
    DesktopArgs {
        action: action.into(),
        region: None,
        image_base64: None,
        x: None,
        y: None,
        button: None,
        text: None,
        keys: None,
        bundle_id: None,
        window_id: None,
        start_x: None,
        start_y: None,
        end_x: None,
        end_y: None,
        delta_x: None,
        delta_y: None,
        duration_ms: None,
        press_action: None,
        duration: None,
        fps: None,
        with_audio: None,
        display_id: None,
        format: None,
        quality: None,
        max_width: None,
        max_height: None,
        actions: None,
    }
}

/// A mock policy that returns a fixed decision for all checks.
struct MockPolicy {
    decision: ApprovalDecision,
}

#[async_trait]
impl ApprovalPolicy for MockPolicy {
    async fn check(&self, _request: &ActionRequest) -> ApprovalDecision {
        self.decision.clone()
    }
    async fn record(&self, _request: &ActionRequest, _decision: &ApprovalDecision) {}
}

#[tokio::test]
async fn test_desktop_approval_deny_blocks_click() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "click blocked".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let mut args = make_args("click");
    args.x = Some(100.0);
    args.y = Some(200.0);
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output.message.as_deref().unwrap().contains("Action denied"));
}

#[tokio::test]
async fn test_desktop_approval_deny_blocks_type_text() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "type blocked".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let mut args = make_args("type_text");
    args.text = Some("secret password".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output.message.as_deref().unwrap().contains("Action denied"));
}

#[tokio::test]
async fn test_desktop_approval_deny_blocks_key_combo() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "key combo blocked".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let mut args = make_args("key_combo");
    args.keys = Some(vec!["cmd".into(), "q".into()]);
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output.message.as_deref().unwrap().contains("Action denied"));
}

#[tokio::test]
async fn test_desktop_approval_deny_blocks_launch_app() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "launch blocked".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let mut args = make_args("launch_app");
    args.bundle_id = Some("com.evil.malware".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output.message.as_deref().unwrap().contains("Action denied"));
}

#[tokio::test]
async fn test_desktop_approval_ask_returns_prompt() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Ask {
            prompt: "Confirm click action".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let mut args = make_args("click");
    args.x = Some(500.0);
    args.y = Some(300.0);
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Approval required"));
    let data = output.data.unwrap();
    assert_eq!(data["approval_required"], true);
}

#[tokio::test]
async fn test_desktop_approval_allows_screenshot() {
    // Screenshot is read-only — should never be blocked even with a
    // deny-all policy. The approval gate is not applied.
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "everything denied".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let args = make_args("screenshot");
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Should NOT be "Action denied". It will fail because no desktop platform
    // capability was wired into this plain test instance.
    assert!(!output.success);
    let msg = output.message.as_deref().unwrap();
    assert!(
        !msg.contains("Action denied"),
        "Read-only action should bypass approval gate, got: {msg}"
    );
}

#[tokio::test]
async fn test_desktop_approval_allows_ocr() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "everything denied".to_string(),
        },
    });
    let tool = DesktopTool::new().with_approval_policy(policy);

    let args = make_args("ocr");
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    let msg = output.message.as_deref().unwrap();
    assert!(
        !msg.contains("Action denied"),
        "Read-only action should bypass approval gate, got: {msg}"
    );
}

#[tokio::test]
async fn test_desktop_no_policy_allows_all() {
    // Without a policy, mutating actions should proceed as before.
    let tool = DesktopTool::new();

    let mut args = make_args("click");
    args.x = Some(100.0);
    args.y = Some(200.0);
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Should fail on missing desktop capability, NOT on approval.
    assert!(!output.success);
    let msg = output.message.as_deref().unwrap();
    assert!(
        !msg.contains("Action denied") && !msg.contains("Approval required"),
        "Without policy, should not hit approval gate, got: {msg}"
    );
    assert!(msg.contains("not configured"));
}

#[tokio::test]
async fn test_desktop_reports_missing_platform_capability() {
    let tool = DesktopTool::new();
    let args = make_args("screenshot");
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not configured"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_desktop_reports_legacy_snapshot_as_unsupported() {
    let tool =
        DesktopTool::new().with_platform(Arc::new(aleph_desktop_macos::MacOSPlatform::new()));
    let args = make_args("snapshot");
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("is not supported on this platform"));
}
