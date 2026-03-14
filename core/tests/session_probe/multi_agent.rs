//! P6: Multi-agent session tests.
//!
//! Covers per-agent session isolation and agent switching scenarios.

use std::sync::Arc;

use alephcore::gateway::router::SessionKey;

use super::harness::SessionProbeHarness;
use super::mock_llm::MockLlmProvider;

// ---------------------------------------------------------------------------
// P6-01: Two agents have independent sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p6_01_two_agents_have_independent_sessions() {
    let h = SessionProbeHarness::new();

    // Create sessions for two different agents
    h.create_session("agent-a").await;
    h.create_session("agent-b").await;

    // Seed different messages to each
    h.seed_messages("agent-a", &[
        ("user", "Hello from agent-a session"),
        ("assistant", "Agent-A reply"),
    ]).await;

    h.seed_messages("agent-b", &[
        ("user", "Hello from agent-b session"),
        ("assistant", "Agent-B reply"),
    ]).await;

    // Verify histories are independent
    let key_a = SessionKey::main("agent-a");
    let key_b = SessionKey::main("agent-b");

    let history_a = h.session_manager.get_history(&key_a, None).await.unwrap();
    let history_b = h.session_manager.get_history(&key_b, None).await.unwrap();

    assert_eq!(history_a.len(), 2, "agent-a should have 2 messages");
    assert_eq!(history_b.len(), 2, "agent-b should have 2 messages");

    // Verify content isolation
    assert!(
        history_a.iter().any(|m| m.content.contains("agent-a")),
        "agent-a history should contain agent-a messages"
    );
    assert!(
        !history_a.iter().any(|m| m.content.contains("agent-b")),
        "agent-a history should NOT contain agent-b messages"
    );
    assert!(
        history_b.iter().any(|m| m.content.contains("agent-b")),
        "agent-b history should contain agent-b messages"
    );
    assert!(
        !history_b.iter().any(|m| m.content.contains("agent-a")),
        "agent-b history should NOT contain agent-a messages"
    );
}

// ---------------------------------------------------------------------------
// P6-02: Closing agent-a's session does not affect agent-b
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p6_02_new_on_agent_a_does_not_affect_agent_b() {
    let h = SessionProbeHarness::new();

    // Create sessions for both agents
    h.create_session("agent-a").await;
    h.create_session("agent-b").await;

    h.seed_messages("agent-a", &[("user", "a-msg")]).await;
    h.seed_messages("agent-b", &[("user", "b-msg")]).await;

    // Close agent-a's session
    let key_a = SessionKey::main("agent-a");
    h.session_manager
        .close_session(&key_a, Some("agent-a topic".to_string()))
        .await
        .expect("Failed to close agent-a session");

    // Verify agent-a is closed
    assert!(
        h.is_session_closed("agent-a").await,
        "agent-a session should be closed"
    );

    // Verify agent-b is NOT closed
    assert!(
        !h.is_session_closed("agent-b").await,
        "agent-b session should NOT be closed"
    );

    // Verify agent-b history is intact
    let key_b = SessionKey::main("agent-b");
    let history_b = h.session_manager.get_history(&key_b, None).await.unwrap();
    assert_eq!(history_b.len(), 1, "agent-b should still have its message");
    assert_eq!(history_b[0].content, "b-msg");
}

// ---------------------------------------------------------------------------
// P6-03: /switch closes only source agent session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p6_03_switch_closes_only_source_agent_session() {
    let llm = Arc::new(MockLlmProvider::new("switch topic"));
    let mut h = SessionProbeHarness::with_llm(Some(llm));

    // Register channel and two agents.
    // "main" is the default agent in RoutingConfig, so DMs route there first.
    h.register_channel("ch-multi").await;
    h.register_agent("main", None).await;
    h.register_agent("beta", None).await;

    // Seed messages for "main" agent via DM helpers (router creates PerPeer keys)
    h.seed_dm_messages("main", &[
        ("user", "Main agent conversation"),
        ("assistant", "Main agent reply"),
    ]).await;

    // Switch from main to beta
    h.send_message("ch-multi", "/switch beta").await;

    // Main agent's DM session should be closed
    assert!(
        h.is_dm_session_closed("main").await,
        "Main agent session should be closed after /switch"
    );

    // Beta should NOT be closed
    assert!(
        !h.is_dm_session_closed("beta").await,
        "Beta session should NOT be closed"
    );
}

// ---------------------------------------------------------------------------
// P6-04: Different peers have independent sessions under same agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p6_04_different_peers_have_independent_sessions() {
    let h = SessionProbeHarness::new();

    // Create PerPeer sessions for different peer IDs under same agent
    let key_peer1 = SessionKey::peer("agent-x", "peer-1");
    let key_peer2 = SessionKey::peer("agent-x", "peer-2");

    h.session_manager
        .get_or_create(&key_peer1)
        .await
        .expect("Failed to create peer-1 session");
    h.session_manager
        .get_or_create(&key_peer2)
        .await
        .expect("Failed to create peer-2 session");

    // Seed different messages
    h.session_manager
        .add_message(&key_peer1, "user", "peer-1 message")
        .await
        .unwrap();
    h.session_manager
        .add_message(&key_peer2, "user", "peer-2 message")
        .await
        .unwrap();

    // Close peer-1's session
    h.session_manager
        .close_session(&key_peer1, Some("peer-1 done".to_string()))
        .await
        .unwrap();

    // Verify peer-1 is closed
    let sessions = h.list_sessions(Some("agent-x")).await;
    let peer1_closed = sessions.iter().any(|s| {
        s.key == key_peer1.to_key_string()
            && s.metadata_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                .and_then(|v| v.get("status")?.as_str().map(|s| s == "closed"))
                .unwrap_or(false)
    });
    assert!(peer1_closed, "peer-1 session should be closed");

    // Verify peer-2 is NOT closed
    let peer2_closed = sessions.iter().any(|s| {
        s.key == key_peer2.to_key_string()
            && s.metadata_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                .and_then(|v| v.get("status")?.as_str().map(|s| s == "closed"))
                .unwrap_or(false)
    });
    assert!(!peer2_closed, "peer-2 session should NOT be closed");

    // Verify peer-2 history is intact
    let history_peer2 = h.session_manager.get_history(&key_peer2, None).await.unwrap();
    assert_eq!(history_peer2.len(), 1);
    assert_eq!(history_peer2[0].content, "peer-2 message");
}

// ---------------------------------------------------------------------------
// P6-05: Epoch increments are agent-scoped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn p6_05_epoch_increments_are_agent_scoped() {
    let h = SessionProbeHarness::new();

    // Create sessions for agent-a with different main_keys (simulating epochs)
    let key_a_s1 = SessionKey::Main {
        agent_id: "agent-a".to_string(),
        main_key: "main:s1".to_string(),
    };
    let key_a_s2 = SessionKey::Main {
        agent_id: "agent-a".to_string(),
        main_key: "main:s2".to_string(),
    };

    // Create agent-b with a single session
    h.create_session("agent-b").await;
    h.seed_messages("agent-b", &[("user", "agent-b original")]).await;

    // Create agent-a epoch sessions
    h.session_manager.get_or_create(&key_a_s1).await.unwrap();
    h.session_manager.add_message(&key_a_s1, "user", "epoch s1").await.unwrap();

    h.session_manager.get_or_create(&key_a_s2).await.unwrap();
    h.session_manager.add_message(&key_a_s2, "user", "epoch s2").await.unwrap();

    // Verify agent-a has multiple sessions
    let sessions_a = h.list_sessions(Some("agent-a")).await;
    assert!(
        sessions_a.len() >= 2,
        "agent-a should have at least 2 sessions (epochs), got {}",
        sessions_a.len()
    );

    // Verify agent-b still only has its original session
    let sessions_b = h.list_sessions(Some("agent-b")).await;
    assert_eq!(
        sessions_b.len(),
        1,
        "agent-b should still have exactly 1 session, got {}",
        sessions_b.len()
    );

    // Verify agent-b's content is unaffected
    let key_b = SessionKey::main("agent-b");
    let history_b = h.session_manager.get_history(&key_b, None).await.unwrap();
    assert_eq!(history_b.len(), 1);
    assert_eq!(history_b[0].content, "agent-b original");
}
