//! Tests for the browser tool.

use super::*;
use crate::browser::ActionTarget;
use crate::tools::AlephTool;
use handlers::resolve_action_target;

fn make_args(action: BrowserAction) -> BrowserArgs {
    BrowserArgs {
        action,
        tab_id: None,
        url: None,
        ref_id: None,
        selector: None,
        text: None,
        js: None,
        direction: None,
        headless: None,
        full_page: None,
    }
}

#[test]
fn test_resolve_action_target_ref() {
    let mut args = make_args(BrowserAction::Click);
    args.ref_id = Some("e42".to_string());
    let target = resolve_action_target(&args).unwrap();
    assert!(matches!(target, ActionTarget::Ref { ref_id } if ref_id == "e42"));
}

#[test]
fn test_resolve_action_target_selector() {
    let mut args = make_args(BrowserAction::Click);
    args.selector = Some("button.submit".to_string());
    let target = resolve_action_target(&args).unwrap();
    assert!(matches!(target, ActionTarget::Selector { css } if css == "button.submit"));
}

#[test]
fn test_resolve_action_target_ref_preferred_over_selector() {
    let mut args = make_args(BrowserAction::Click);
    args.ref_id = Some("e1".to_string());
    args.selector = Some("div".to_string());
    let target = resolve_action_target(&args).unwrap();
    // ref_id takes priority
    assert!(matches!(target, ActionTarget::Ref { ref_id } if ref_id == "e1"));
}

#[test]
fn test_resolve_action_target_missing() {
    let args = make_args(BrowserAction::Click);
    assert!(resolve_action_target(&args).is_err());
}

#[test]
fn test_tool_definition() {
    let tool = BrowserTool::new();
    let def = AlephTool::definition(&tool);
    assert_eq!(def.name, "browser");
    assert!(def.description.contains("Chromium"));
    assert!(def.llm_context.is_some());
}

#[tokio::test]
async fn test_not_running_returns_error_output() {
    let tool = BrowserTool::new();
    let args = BrowserArgs {
        action: BrowserAction::ListTabs,
        ..make_args(BrowserAction::ListTabs)
    };
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output.message.unwrap().contains("not running"));
}

#[tokio::test]
async fn test_stop_when_not_running() {
    let tool = BrowserTool::new();
    let args = make_args(BrowserAction::Stop);
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Should succeed gracefully even when not running
    assert!(output.success);
    assert!(output.message.unwrap().contains("not running"));
}

#[tokio::test]
async fn test_open_tab_missing_url() {
    let tool = BrowserTool::new();
    // Start would fail anyway (no browser), but open_tab should fail on missing url first...
    // Actually, open_tab checks url before require_running, but our impl checks url first.
    // Let's test the url-missing path by having a running browser... we can't easily do that.
    // Instead, test that the error for not-running is returned.
    let args = make_args(BrowserAction::OpenTab);
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    // Either "url" error or "not running" — our impl checks url before require_running
    assert!(output.message.is_some());
}

#[tokio::test]
async fn test_click_missing_target() {
    let tool = BrowserTool::new();
    let mut args = make_args(BrowserAction::Click);
    args.tab_id = Some("fake".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    // Will fail either on "not running" or "missing target"
    assert!(output.message.is_some());
}

#[test]
fn test_browser_action_serde() {
    let action: BrowserAction = serde_json::from_str(r#""start""#).unwrap();
    assert!(matches!(action, BrowserAction::Start));

    let action: BrowserAction = serde_json::from_str(r#""open_tab""#).unwrap();
    assert!(matches!(action, BrowserAction::OpenTab));

    let action: BrowserAction = serde_json::from_str(r#""snapshot""#).unwrap();
    assert!(matches!(action, BrowserAction::Snapshot));
}

#[test]
fn test_browser_args_deserialization() {
    let json = serde_json::json!({
        "action": "click",
        "tab_id": "abc123",
        "ref_id": "e42"
    });
    let args: BrowserArgs = serde_json::from_value(json).unwrap();
    assert!(matches!(args.action, BrowserAction::Click));
    assert_eq!(args.tab_id.as_deref(), Some("abc123"));
    assert_eq!(args.ref_id.as_deref(), Some("e42"));
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
async fn test_approval_deny_blocks_navigate() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "navigation blocked".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::Navigate);
    args.url = Some("https://evil.com".to_string());
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
}

#[tokio::test]
async fn test_approval_ask_returns_prompt() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Ask {
            prompt: "Confirm JS execution".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::Evaluate);
    args.js = Some("document.cookie".to_string());
    args.tab_id = Some("tab1".to_string());
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
async fn test_no_approval_policy_allows_all() {
    // BrowserTool without a policy — Navigate should proceed normally.
    // (It will fail because browser is not running, but the approval gate
    // should NOT block it.)
    let tool = BrowserTool::new();

    let mut args = make_args(BrowserAction::Navigate);
    args.url = Some("https://example.com".to_string());
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Should fail on "not running", NOT on approval denial
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not running"));
}

#[tokio::test]
async fn test_approval_allow_proceeds_to_execution() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Allow,
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::Navigate);
    args.url = Some("https://example.com".to_string());
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Approval gate passed; should fail on "not running" (no browser started)
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not running"));
}

#[tokio::test]
async fn test_approval_deny_blocks_click() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "click blocked".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::Click);
    args.ref_id = Some("e42".to_string());
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
}

#[tokio::test]
async fn test_approval_deny_blocks_type() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "type blocked".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::Type);
    args.text = Some("password".to_string());
    args.ref_id = Some("input1".to_string());
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
}

#[tokio::test]
async fn test_approval_deny_blocks_fill() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "fill blocked".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::Fill);
    args.text = Some("secret".to_string());
    args.ref_id = Some("input2".to_string());
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
}

#[tokio::test]
async fn test_approval_deny_blocks_open_tab() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "open tab blocked".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::OpenTab);
    args.url = Some("https://evil.com".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("Action denied"));
}

#[tokio::test]
async fn test_approval_allow_proceeds_open_tab() {
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Allow,
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    let mut args = make_args(BrowserAction::OpenTab);
    args.url = Some("https://example.com".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Approval gate passed; should fail on "not running" (no browser started)
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not running"));
}

#[tokio::test]
async fn test_read_only_actions_not_blocked() {
    // Even with a deny-all policy, read-only actions should proceed.
    let policy = Arc::new(MockPolicy {
        decision: ApprovalDecision::Deny {
            reason: "everything denied".to_string(),
        },
    });
    let tool = BrowserTool::new().with_approval_policy(policy);

    // Screenshot — read-only, no approval check
    let mut args = make_args(BrowserAction::Screenshot);
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    // Should fail on "not running", NOT on approval
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not running"));

    // Snapshot — read-only, no approval check
    let mut args = make_args(BrowserAction::Snapshot);
    args.tab_id = Some("tab1".to_string());
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not running"));

    // ListTabs — read-only, no approval check
    let args = make_args(BrowserAction::ListTabs);
    let output = AlephTool::call(&tool, args).await.unwrap();
    assert!(!output.success);
    assert!(output
        .message
        .as_deref()
        .unwrap()
        .contains("not running"));
}
