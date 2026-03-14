//! P4: Intent/switch edge cases — 5 scenarios.
//!
//! Since IntentDetector requires an LLM provider (not available in tests),
//! these scenarios test additional /switch and routing edge cases.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p4_01_switch_allowed_basic() {
    // Basic /switch to an allowed agent verifies the full path
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("agent-a", Some(vec!["tg-1".into()])).await;
    h.send_message("tg-1", "/switch agent-a").await;
    let replies = h.drain_replies();
    assert!(replies.iter().any(|r| r.text.contains("✅")));
}

#[tokio::test]
async fn p4_02_switch_denied_gets_error() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("agent-b", Some(vec!["dc-1".into()])).await;
    h.send_message("tg-1", "/switch agent-b").await;
    let replies = h.drain_replies();
    assert!(replies.iter().any(|r| r.text.contains('\u{26D4}')));
    assert!(replies.iter().any(|r| r.text.contains("tg-1") && r.text.contains("agent-b")));
}

#[tokio::test]
async fn p4_03_denied_switch_no_state_change() {
    // After denied switch, active agent should remain unchanged
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("denied-agent", Some(vec!["dc-only".into()])).await;

    h.send_message("tg-1", "/switch denied-agent").await;
    h.drain_replies();

    // Verify workspace_manager still has no explicit switch (or still "main")
    // Send a normal message — should route to "main" and not be denied
    h.send_message("tg-1", "still on main").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p4_04_sequential_switches() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("agent-a", Some(vec!["tg-1".into()])).await;
    h.register_agent("agent-b", Some(vec!["tg-1".into()])).await;

    h.send_message("tg-1", "/switch agent-a").await;
    let r1 = h.drain_replies();
    assert!(r1.iter().any(|r| r.text.contains("✅") && r.text.contains("agent-a")));

    h.send_message("tg-1", "/switch agent-b").await;
    let r2 = h.drain_replies();
    assert!(r2.iter().any(|r| r.text.contains("✅") && r.text.contains("agent-b")));
}

#[tokio::test]
async fn p4_05_normal_message_no_intent_check() {
    // Without intent detector, normal messages just route normally (no ACL on non-existent target)
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.send_message("tg-1", "please use agent-x to write a report").await;
    h.assert_no_denial(); // No intent detection → normal routing → "main" allows all
}
