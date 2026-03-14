//! P5: Config Lifecycle — 5 scenarios.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p5_01_runtime_restrict() {
    // Agent starts with None (all allowed), update to restricted, next message denied
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;

    h.send_message("tg-1", "before restriction").await;
    h.assert_no_denial();

    h.update_allowed_links("main", Some(vec!["dc-1".into()])).await;
    h.send_message("tg-1", "after restriction").await;
    h.assert_denied();
}

#[tokio::test]
async fn p5_02_runtime_unrestrict() {
    // Agent starts restricted, update to None, next message allowed
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await;

    h.send_message("tg-1", "while restricted").await;
    h.assert_denied();

    h.update_allowed_links("main", None).await;
    h.send_message("tg-1", "after unrestrict").await;
    h.assert_no_denial();
}

#[tokio::test]
async fn p5_03_runtime_change_list() {
    // Agent allows link-A, update to allow only link-B → link-A now denied
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["tg-1".into()])).await;

    h.send_message("tg-1", "initially allowed").await;
    h.assert_no_denial();

    h.update_allowed_links("main", Some(vec!["dc-1".into()])).await;
    h.send_message("tg-1", "now denied").await;
    h.assert_denied();
}

#[tokio::test]
async fn p5_04_new_agent_default_none() {
    // Newly created agent has allowed_links=None → all links can access
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("new-agent", None).await;

    h.send_message("tg-1", "/switch new-agent").await;
    let replies = h.drain_replies();
    assert!(
        replies.iter().any(|r| r.text.contains("✅")),
        "New agent with None should allow switch"
    );
}

#[tokio::test]
async fn p5_05_delete_recreate_agent() {
    // Delete agent, recreate with different allowed_links → new config effective
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("target", Some(vec!["tg-1".into()])).await;

    // Switch works
    h.send_message("tg-1", "/switch target").await;
    let r1 = h.drain_replies();
    assert!(r1.iter().any(|r| r.text.contains("✅")));

    // Switch back to main
    h.send_message("tg-1", "/switch main").await;
    h.drain_replies();

    // Recreate target with different ACL (deny tg-1)
    h.agent_registry.remove("target").await;
    h.register_agent("target", Some(vec!["dc-only".into()])).await;

    // Now switch should be denied
    h.send_message("tg-1", "/switch target").await;
    h.assert_denied();
}
