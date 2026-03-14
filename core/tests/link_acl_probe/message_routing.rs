//! P2: Message Routing enforcement — 8 scenarios.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p2_01_unrestricted_agent_any_link() {
    let mut h = LinkAclHarness::new();
    h.register_link("any-bot").await;
    h.register_agent("main", None).await;
    h.send_message("any-bot", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p2_02_restricted_agent_allowed_link() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["tg-1".into()])).await;
    h.send_message("tg-1", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p2_03_restricted_agent_denied_link() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;
    h.send_message("tg-1", "hello").await;
    h.assert_denied();
}

#[tokio::test]
async fn p2_04_agent_not_in_registry() {
    // When agent is not in registry, ACL check is skipped (graceful)
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    // Don't register any agent — default "main" doesn't exist
    h.send_message("tg-1", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p2_05_denied_message_not_executed() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;
    h.send_message("tg-1", "hello").await;
    h.assert_denied();
    assert!(!h.tracking_adapter.was_called(), "Execution should NOT be called for denied message");
}

#[tokio::test]
async fn p2_06_denial_reply_format() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;
    h.send_message("tg-1", "hello").await;
    let replies = h.drain_replies();
    let denial = replies.iter().find(|r| r.text.contains('\u{26D4}')).expect("Should have denial reply");
    assert!(denial.text.contains("tg-1"), "Denial should contain link_id");
    assert!(denial.text.contains("main"), "Denial should contain agent_id");
}

#[tokio::test]
async fn p2_07_consecutive_denials() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;
    // Send twice — both should be denied (no cache/bypass)
    h.send_message("tg-1", "first").await;
    h.send_message("tg-1", "second").await;
    let replies = h.drain_replies();
    let denials: Vec<_> = replies.iter().filter(|r| r.text.contains('\u{26D4}')).collect();
    assert_eq!(denials.len(), 2, "Both messages should be denied, got {} denials", denials.len());
}

#[tokio::test]
async fn p2_08_group_message_acl() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;
    h.send_group_message("tg-1", "hello group").await;
    h.assert_denied();
}
