//! Tests for the desktop tool.

use super::*;
use crate::desktop::types::{MouseButton, ScreenRegion};
use crate::desktop::DesktopRequest;
use crate::tools::AlephTool;

fn make_args(action: &str) -> DesktopArgs {
    DesktopArgs {
        action: action.into(),
        region: None,
        image_base64: None,
        app_bundle_id: None,
        x: None,
        y: None,
        button: None,
        text: None,
        keys: None,
        bundle_id: None,
        window_id: None,
        html: None,
        position: None,
        patch: None,
        ref_id: None,
        start_ref: None,
        start_x: None,
        start_y: None,
        end_ref: None,
        end_x: None,
        end_y: None,
        delta_x: None,
        delta_y: None,
        duration_ms: None,
        max_depth: None,
        include_non_interactive: None,
        duration: None,
        fps: None,
        with_audio: None,
    }
}

#[test]
fn test_build_request_screenshot() {
    let args = make_args("screenshot");
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Screenshot { region: None }));
}

#[test]
fn test_build_request_screenshot_with_region() {
    let mut args = make_args("screenshot");
    args.region = Some(ScreenRegion { x: 10.0, y: 20.0, width: 100.0, height: 200.0 });
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Screenshot { region: Some(_) }));
}

#[test]
fn test_build_request_ocr() {
    let args = make_args("ocr");
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Ocr { image_base64: None }));
}

#[test]
fn test_build_request_click() {
    let mut args = make_args("click");
    args.x = Some(100.0);
    args.y = Some(200.0);
    args.button = Some(MouseButton::Right);
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Click { ref_id: None, button: MouseButton::Right, .. }));
}

#[test]
fn test_build_request_window_list() {
    let args = make_args("window_list");
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::WindowList));
}

#[test]
fn test_build_request_canvas_hide() {
    let args = make_args("canvas_hide");
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::CanvasHide));
}

#[test]
fn test_build_request_key_combo() {
    let mut args = make_args("key_combo");
    args.keys = Some(vec!["cmd".into(), "c".into()]);
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::KeyCombo { .. }));
}

#[test]
fn test_build_request_canvas_show_default_position() {
    let mut args = make_args("canvas_show");
    args.html = Some("<h1>Hello</h1>".into());
    // No position supplied — should use the default 100/100/800/600
    let req = request::build_request(&args).unwrap();
    if let DesktopRequest::CanvasShow { position, .. } = req {
        assert_eq!(position.x, 100.0);
        assert_eq!(position.width, 800.0);
    } else {
        panic!("expected CanvasShow");
    }
}

#[test]
fn test_build_request_unknown_action() {
    let args = make_args("unknown");
    assert!(request::build_request(&args).is_err());
}

#[test]
fn test_build_request_unknown_action_message() {
    let args = make_args("fly");
    let err = request::build_request(&args).unwrap_err();
    assert!(err.contains("fly"), "error should mention the unknown action");
}

#[test]
fn test_build_request_snapshot() {
    let mut args = make_args("snapshot");
    args.max_depth = Some(3);
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Snapshot { max_depth: Some(3), .. }));
}

#[test]
fn test_build_request_click_with_ref() {
    let mut args = make_args("click");
    args.ref_id = Some("e3".into());
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Click { ref_id: Some(_), .. }));
}

#[test]
fn test_build_request_click_no_target() {
    let args = make_args("click");
    assert!(request::build_request(&args).is_err());
}

#[test]
fn test_build_request_scroll() {
    let mut args = make_args("scroll");
    args.ref_id = Some("e7".into());
    args.delta_y = Some(-300.0);
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Scroll { .. }));
}

#[test]
fn test_build_request_double_click() {
    let mut args = make_args("double_click");
    args.ref_id = Some("e1".into());
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::DoubleClick { .. }));
}

#[test]
fn test_build_request_drag() {
    let mut args = make_args("drag");
    args.start_ref = Some("e1".into());
    args.end_ref = Some("e5".into());
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Drag { .. }));
}

#[test]
fn test_build_request_drag_missing_end() {
    let mut args = make_args("drag");
    args.start_ref = Some("e1".into());
    assert!(request::build_request(&args).is_err());
}

#[test]
fn test_build_request_hover() {
    let mut args = make_args("hover");
    args.x = Some(100.0);
    args.y = Some(200.0);
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Hover { .. }));
}

#[test]
fn test_build_request_paste() {
    let mut args = make_args("paste");
    args.text = Some("hello".into());
    let req = request::build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Paste { text } if text == "hello"));
}

// ── Approval policy tests ──────────────────────────────────────────

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};

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
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
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
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
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
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
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
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
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
    // Should NOT be "Action denied". It will fail on bridge/app not available,
    // which is the expected behavior (approval gate was not triggered).
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
    // Should fail on bridge/app not available, NOT on approval
    assert!(!output.success);
    let msg = output.message.as_deref().unwrap();
    assert!(
        !msg.contains("Action denied") && !msg.contains("Approval required"),
        "Without policy, should not hit approval gate, got: {msg}"
    );
}
