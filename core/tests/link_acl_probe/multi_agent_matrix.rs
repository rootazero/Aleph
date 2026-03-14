//! P6: Multi-Agent Matrix — 6 scenarios.

use super::harness::LinkAclHarness;

#[tokio::test]
async fn p6_01_three_by_three_matrix() {
    // 3 agents × 3 links with different permissions
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_link("dc-1").await;
    h.register_link("sl-1").await;

    h.register_agent("main", None).await; // default, all allowed
    h.register_agent("agent-a", Some(vec!["tg-1".into(), "dc-1".into()])).await;
    h.register_agent("agent-b", Some(vec!["dc-1".into(), "sl-1".into()])).await;

    // tg-1 → agent-a: allowed
    h.send_message("tg-1", "/switch agent-a").await;
    let r = h.drain_replies();
    assert!(
        r.iter().any(|r| r.text.contains("✅")),
        "tg-1 → agent-a should succeed"
    );
    h.send_message("tg-1", "/switch main").await;
    h.drain_replies();

    // sl-1 → agent-a: denied
    h.send_message("sl-1", "/switch agent-a").await;
    let r = h.drain_replies();
    assert!(
        r.iter().any(|r| r.text.contains('\u{26D4}')),
        "sl-1 → agent-a should be denied"
    );

    // dc-1 → agent-b: allowed
    h.send_message("dc-1", "/switch agent-b").await;
    let r = h.drain_replies();
    assert!(
        r.iter().any(|r| r.text.contains("✅")),
        "dc-1 → agent-b should succeed"
    );
    h.send_message("dc-1", "/switch main").await;
    h.drain_replies();

    // tg-1 → agent-b: denied
    h.send_message("tg-1", "/switch agent-b").await;
    let r = h.drain_replies();
    assert!(
        r.iter().any(|r| r.text.contains('\u{26D4}')),
        "tg-1 → agent-b should be denied"
    );
}

#[tokio::test]
async fn p6_02_default_agent_restricted() {
    // Default agent has restricted list → message from denied link gets denied
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", Some(vec!["dc-1".into()])).await; // main restricts tg-1
    h.send_message("tg-1", "hello").await;
    h.assert_denied();
}

#[tokio::test]
async fn p6_03_independent_agents() {
    // Agent-A allows link-1, Agent-B allows link-2 → no cross-contamination
    let mut h = LinkAclHarness::new();
    h.register_link("link-1").await;
    h.register_link("link-2").await;
    h.register_agent("main", None).await;
    h.register_agent("agent-a", Some(vec!["link-1".into()])).await;
    h.register_agent("agent-b", Some(vec!["link-2".into()])).await;

    // link-1 can access agent-a
    h.send_message("link-1", "/switch agent-a").await;
    let r = h.drain_replies();
    assert!(r.iter().any(|r| r.text.contains("✅")));
    h.send_message("link-1", "/switch main").await;
    h.drain_replies();

    // link-1 cannot access agent-b
    h.send_message("link-1", "/switch agent-b").await;
    let r = h.drain_replies();
    assert!(r.iter().any(|r| r.text.contains('\u{26D4}')));

    // link-2 can access agent-b
    h.send_message("link-2", "/switch agent-b").await;
    let r = h.drain_replies();
    assert!(r.iter().any(|r| r.text.contains("✅")));
    h.send_message("link-2", "/switch main").await;
    h.drain_replies();

    // link-2 cannot access agent-a
    h.send_message("link-2", "/switch agent-a").await;
    let r = h.drain_replies();
    assert!(r.iter().any(|r| r.text.contains('\u{26D4}')));
}

#[tokio::test]
async fn p6_04_same_link_different_agents() {
    // One link has different permissions on different agents
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await;
    h.register_agent("open-agent", Some(vec!["tg-1".into()])).await;
    h.register_agent("closed-agent", Some(vec!["dc-1".into()])).await;

    h.send_message("tg-1", "/switch open-agent").await;
    let r = h.drain_replies();
    assert!(
        r.iter().any(|r| r.text.contains("✅")),
        "tg-1 → open-agent should succeed"
    );
    h.send_message("tg-1", "/switch main").await;
    h.drain_replies();

    h.send_message("tg-1", "/switch closed-agent").await;
    let r = h.drain_replies();
    assert!(
        r.iter().any(|r| r.text.contains('\u{26D4}')),
        "tg-1 → closed-agent should be denied"
    );
}

#[tokio::test]
async fn p6_05_all_agents_deny_link() {
    // Every agent denies a specific link → that link cannot access anything
    let mut h = LinkAclHarness::new();
    h.register_link("banned-bot").await;
    h.register_agent("main", Some(vec!["tg-1".into()])).await;
    h.register_agent("helper", Some(vec!["tg-1".into()])).await;

    // banned-bot can't even send regular messages (main denies it)
    h.send_message("banned-bot", "hello").await;
    h.assert_denied();
}

#[tokio::test]
async fn p6_06_mixed_policies() {
    // One agent allows all (None), another restricts → both work simultaneously
    let mut h = LinkAclHarness::new();
    h.register_link("tg-1").await;
    h.register_agent("main", None).await; // allows all
    h.register_agent("restricted", Some(vec!["dc-1".into()])).await;

    // tg-1 to main: OK (no denial)
    h.send_message("tg-1", "hello main").await;
    h.assert_no_denial();

    // tg-1 try to switch to restricted: denied
    h.send_message("tg-1", "/switch restricted").await;
    h.assert_denied();
}
