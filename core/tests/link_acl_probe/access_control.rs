//! P1: Access Control pure logic — 6 scenarios.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p1_01_none_allows_all() {
    let mut h = LinkAclHarness::new();
    h.register_link("telegram-bot").await;
    h.register_agent("main", None).await; // None = all allowed
    h.send_message("telegram-bot", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p1_02_empty_list_allows_all() {
    let mut h = LinkAclHarness::new();
    h.register_link("telegram-bot").await;
    h.register_agent("main", Some(vec![])).await; // empty = all allowed
    h.send_message("telegram-bot", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p1_03_whitelist_hit() {
    let mut h = LinkAclHarness::new();
    h.register_link("telegram-bot").await;
    h.register_agent("main", Some(vec!["telegram-bot".into()])).await;
    h.send_message("telegram-bot", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p1_04_whitelist_miss() {
    let mut h = LinkAclHarness::new();
    h.register_link("discord-bot").await;
    h.register_agent("main", Some(vec!["telegram-bot".into()])).await;
    h.send_message("discord-bot", "hello").await;
    h.assert_denied();
}

#[tokio::test]
async fn p1_05_multi_link_whitelist() {
    let mut h = LinkAclHarness::new();
    h.register_link("tg-2").await;
    h.register_agent("main", Some(vec!["tg-1".into(), "tg-2".into()])).await;
    h.send_message("tg-2", "hello").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p1_06_single_link_rejects_others() {
    let mut h = LinkAclHarness::new();
    h.register_link("dc-1").await;
    h.register_agent("main", Some(vec!["tg-1".into()])).await;
    h.send_message("dc-1", "hello").await;
    // Drain manually to check both denial and content
    let replies = h.drain_replies();
    assert!(
        replies.iter().any(|r| r.text.contains('\u{26D4}') || r.text.to_lowercase().contains("not allowed")),
        "Expected denial, got: {:?}", replies.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
    // Verify the error message contains both link_id and agent_id
    assert!(
        replies.iter().any(|r| r.text.contains("dc-1")),
        "Denial should mention link_id 'dc-1'"
    );
    assert!(
        replies.iter().any(|r| r.text.contains("main")),
        "Denial should mention agent_id 'main'"
    );
}
