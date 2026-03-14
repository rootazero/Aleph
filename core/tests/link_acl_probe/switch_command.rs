//! P3: /switch command enforcement — 6 scenarios.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p3_01_switch_to_allowed_agent() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await; // default agent, all links allowed
    h.register_agent("helper", Some(vec!["tg-1".into()])).await;
    h.send_message("tg-1", "/switch helper").await;
    let replies = h.drain_replies();
    assert!(
        replies.iter().any(|r| r.text.contains("✅") && r.text.contains("helper")),
        "Expected switch success, got: {:?}", replies.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn p3_02_switch_to_denied_agent() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("private", Some(vec!["dc-1".into()])).await; // denies tg-1
    h.send_message("tg-1", "/switch private").await;
    h.assert_denied();
}

#[tokio::test]
async fn p3_03_switch_to_nonexistent_agent() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.send_message("tg-1", "/switch ghost").await;
    let replies = h.drain_replies();
    assert!(
        replies.iter().any(|r| r.text.contains("not found") || r.text.contains("❌")),
        "Expected 'not found', got: {:?}", replies.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn p3_04_switch_back_to_allowed() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("helper", Some(vec!["tg-1".into()])).await;
    // Switch to helper
    h.send_message("tg-1", "/switch helper").await;
    h.drain_replies(); // clear
    // Switch back to main
    h.send_message("tg-1", "/switch main").await;
    let replies = h.drain_replies();
    assert!(
        replies.iter().any(|r| r.text.contains("✅") && r.text.contains("main")),
        "Expected switch back success, got: {:?}", replies.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn p3_05_different_links_different_permissions() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_link("dc-1").await;
    h.register_agent("main", None).await;
    h.register_agent("restricted", Some(vec!["tg-1".into()])).await; // only tg-1

    // tg-1 can switch
    h.send_message("tg-1", "/switch restricted").await;
    let replies = h.drain_replies();
    assert!(replies.iter().any(|r| r.text.contains("✅")), "tg-1 should succeed");

    // dc-1 cannot switch
    h.send_message("dc-1", "/switch restricted").await;
    h.assert_denied();
}

#[tokio::test]
async fn p3_06_denied_switch_preserves_current() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("private", Some(vec!["dc-1".into()])).await;

    // Try to switch to denied agent
    h.send_message("tg-1", "/switch private").await;
    h.drain_replies(); // clear denial

    // Send normal message — should still route to "main" (not "private")
    h.send_message("tg-1", "hello after denied switch").await;
    h.assert_no_denial(); // "main" allows all, so no denial
}
