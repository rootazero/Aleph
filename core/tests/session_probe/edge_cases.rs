//! P7: Edge case tests.
//!
//! Covers concurrent access, corrupt metadata, large histories,
//! and other boundary conditions.

use super::harness::SessionProbeHarness;
use alephcore::gateway::router::SessionKey;
use alephcore::routing::session_key::SessionKey as RoutingKey;

// =============================================================================
// P7 tests
// =============================================================================

/// Close a session that was never created. Should not panic.
#[tokio::test]
async fn p7_01_close_nonexistent_session_no_panic() {
    let h = SessionProbeHarness::new();

    let key = SessionKey::Main {
        agent_id: "nonexistent".to_string(),
        main_key: "main".to_string(),
    };

    // Should not panic — OK if it succeeds or returns an error
    let _ = h
        .session_manager
        .close_session(&key, Some("topic".to_string()))
        .await;
}

/// Close same session twice with different topics; second topic overwrites first.
#[tokio::test]
async fn p7_02_close_session_twice_is_idempotent() {
    let h = SessionProbeHarness::new();

    // Create the session first
    h.create_session("agent-a").await;

    let key = SessionKey::Main {
        agent_id: "agent-a".to_string(),
        main_key: "main".to_string(),
    };

    h.session_manager
        .close_session(&key, Some("first topic".to_string()))
        .await
        .expect("First close should succeed");

    h.session_manager
        .close_session(&key, Some("second topic".to_string()))
        .await
        .expect("Second close should succeed");

    // Second topic should overwrite first
    let topic = h.get_topic("agent-a").await;
    assert_eq!(topic, Some("second topic".to_string()));
}

/// Create empty session (0 messages), close it, verify closed with no topic.
#[tokio::test]
async fn p7_03_empty_session_new_works_gracefully() {
    let h = SessionProbeHarness::new();

    h.create_session("empty-agent").await;

    let key = SessionKey::Main {
        agent_id: "empty-agent".to_string(),
        main_key: "main".to_string(),
    };

    // Close without topic
    h.session_manager
        .close_session(&key, None)
        .await
        .expect("Close empty session should succeed");

    // Should be closed
    assert!(h.is_session_closed("empty-agent").await);

    // No topic set
    let topic = h.get_topic("empty-agent").await;
    assert!(topic.is_none());
}

/// `RoutingKey::main("test").to_key_string()` should NOT contain `:s0` or `:s`.
#[test]
fn p7_04_epoch_zero_no_s0_suffix() {
    let key = RoutingKey::main("test");
    let key_str = key.to_key_string();

    assert!(
        !key_str.contains(":s0"),
        "Epoch 0 should not produce :s0 suffix, got: {}",
        key_str
    );
    assert!(
        !key_str.ends_with(":s"),
        "Key should not end with bare :s suffix, got: {}",
        key_str
    );
    assert_eq!(key_str, "agent:test:main");
}

/// `RoutingKey::parse("agent:test:main")` has epoch 0. Also test DM key.
#[test]
fn p7_05_legacy_key_without_epoch_defaults_to_zero() {
    let main_key = RoutingKey::parse("agent:test:main").expect("Should parse main key");
    assert_eq!(main_key.epoch(), 0);

    let dm_key = RoutingKey::parse("agent:test:dm:user1").expect("Should parse DM key");
    assert_eq!(dm_key.epoch(), 0);
}

/// `get_current_epoch("agent:nonexistent:main")` returns 0.
#[tokio::test]
async fn p7_06_get_current_epoch_returns_zero_for_nonexistent() {
    let h = SessionProbeHarness::new();

    let epoch = h
        .session_manager
        .get_current_epoch("agent:nonexistent:main")
        .await
        .expect("Should not error");

    assert_eq!(epoch, 0);
}

/// Create session but don't close it; `get_topic()` returns None.
#[tokio::test]
async fn p7_07_get_topic_returns_none_for_session_without_metadata() {
    let h = SessionProbeHarness::new();

    h.create_session("no-topic-agent").await;

    let topic = h.get_topic("no-topic-agent").await;
    assert!(
        topic.is_none(),
        "Topic should be None for session without metadata, got: {:?}",
        topic
    );
}

/// Spawn 10 concurrent tokio tasks creating different agent sessions;
/// verify all 10 exist afterward.
#[tokio::test]
async fn p7_08_concurrent_session_creation_no_corruption() {
    let h = SessionProbeHarness::new();
    let manager = h.session_manager.clone();

    let mut handles = Vec::new();
    for i in 0..10 {
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            let key = SessionKey::Main {
                agent_id: format!("concurrent-{}", i),
                main_key: "main".to_string(),
            };
            mgr.get_or_create(&key)
                .await
                .expect("Concurrent session creation should succeed");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.expect("Task should not panic");
    }

    // Verify all 10 sessions exist
    let all_sessions = h.list_sessions(None).await;
    for i in 0..10 {
        let agent_id = format!("concurrent-{}", i);
        assert!(
            all_sessions.iter().any(|s| s.agent_id == agent_id),
            "Session for {} should exist",
            agent_id
        );
    }
}
