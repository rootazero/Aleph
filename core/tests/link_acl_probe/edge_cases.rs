//! P7: Edge Cases + System Interaction — 7 scenarios.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p7_01_stale_link_in_whitelist() {
    // allowed_links contains "deleted-bot" (not registered) → other links still checked correctly
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["deleted-bot".into(), "tg-1".into()])).await;
    h.send_message("tg-1", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p7_02_empty_link_id() {
    // Message from link with empty string id → safe handling
    let mut h = LinkAclHarness::new();
    h.register_link("").await;
    h.register_agent("main", Some(vec!["tg-1".into()])).await;
    h.send_message("", "hello").await;
    // Empty link id not in whitelist → should be denied
    h.assert_denied();
}

#[tokio::test]
async fn p7_03_duplicate_entries() {
    // allowed_links has duplicate entries → works correctly, no panic
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["tg-1".into(), "tg-1".into()])).await;
    h.send_message("tg-1", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p7_04_empty_agent_id() {
    // Agent with empty string id → safe handling
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("", Some(vec!["tg-1".into()])).await;
    // Try switching to empty-id agent
    h.send_message("tg-1", "/switch ").await;
    // Should not panic — either "not found" or handled gracefully
    let _ = h.drain_replies();
}

#[tokio::test]
async fn p7_05_message_routing_then_acl() {
    // Routing resolves to agent, then ACL blocks → ACL takes precedence
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("restricted", Some(vec!["dc-1".into()])).await;

    // Set active agent to restricted via workspace_manager
    h.workspace_manager
        .set_active_agent("tg-1", "user-1", "restricted")
        .unwrap();

    // Now message from tg-1 resolves to "restricted" which denies tg-1
    h.send_message("tg-1", "should be denied by ACL").await;
    h.assert_denied();
}

#[tokio::test]
async fn p7_06_switch_denied_acl_overrides_routing() {
    // AgentRouter would route to agent-X, but ACL denies → denied
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("target", Some(vec!["dc-only".into()])).await;

    h.send_message("tg-1", "/switch target").await;
    h.assert_denied();
}

#[tokio::test]
async fn p7_07_concurrent_denials() {
    // Multiple concurrent messages from denied link → all denied, no race
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;

    // Send 5 messages rapidly
    for i in 0..5 {
        h.send_message("tg-1", &format!("concurrent-{}", i)).await;
    }

    let replies = h.drain_replies();
    let denials: Vec<_> = replies
        .iter()
        .filter(|r| r.text.contains('\u{26D4}') || r.text.to_lowercase().contains("not allowed"))
        .collect();
    assert_eq!(
        denials.len(),
        5,
        "All 5 messages should be denied, got {}",
        denials.len()
    );
}
