//! P1: Session lifecycle tests.
//!
//! Covers creation, close, topic, and epoch scenarios.

use super::harness::SessionProbeHarness;
use alephcore::gateway::router::SessionKey;

// =============================================================================
// Helpers
// =============================================================================

/// Build a SessionKey with a given epoch suffix to simulate epoch transitions.
fn epoch_key(agent_id: &str, epoch: u32) -> SessionKey {
    SessionKey::Main {
        agent_id: agent_id.to_string(),
        main_key: format!("main:s{}", epoch),
    }
}

// =============================================================================
// P1 tests
// =============================================================================

#[tokio::test]
async fn p1_01_create_session_returns_metadata() {
    let h = SessionProbeHarness::new();
    let meta = h.create_session("test").await;

    assert_eq!(meta.agent_id, "test");
    assert_eq!(meta.session_type, "main");
    assert_eq!(meta.message_count, 0);
    assert!(meta.created_at > 0);
    assert!(meta.last_active_at > 0);
}

#[tokio::test]
async fn p1_02_close_session_sets_closed_status() {
    let h = SessionProbeHarness::new();
    let _meta = h.create_session("agent-close").await;

    let key = SessionKey::main("agent-close");
    h.session_manager.close_session(&key, None).await.unwrap();

    assert!(h.is_session_closed("agent-close").await);
}

#[tokio::test]
async fn p1_03_close_with_topic_stores_topic() {
    let h = SessionProbeHarness::new();
    let _meta = h.create_session("agent-topic").await;

    let key = SessionKey::main("agent-topic");
    h.session_manager
        .close_session(&key, Some("Rust并发讨论".to_string()))
        .await
        .unwrap();

    let topic = h.get_topic("agent-topic").await;
    assert_eq!(topic, Some("Rust并发讨论".to_string()));
}

#[tokio::test]
async fn p1_04_close_without_topic_leaves_topic_none() {
    let h = SessionProbeHarness::new();
    let _meta = h.create_session("agent-notopic").await;

    let key = SessionKey::main("agent-notopic");
    h.session_manager.close_session(&key, None).await.unwrap();

    let topic = h.get_topic("agent-notopic").await;
    assert!(topic.is_none());
}

#[tokio::test]
async fn p1_05_new_epoch_creates_separate_session() {
    let h = SessionProbeHarness::new();

    let key0 = epoch_key("agent-epoch", 0);
    let key1 = epoch_key("agent-epoch", 1);

    h.session_manager.get_or_create(&key0).await.unwrap();
    h.session_manager.get_or_create(&key1).await.unwrap();

    let sessions = h.list_sessions(Some("agent-epoch")).await;
    assert_eq!(sessions.len(), 2);
}

#[tokio::test]
async fn p1_06_old_session_retains_messages_after_new_epoch() {
    let h = SessionProbeHarness::new();

    // Seed messages in epoch 0
    let key0 = epoch_key("agent-retain", 0);
    h.session_manager.get_or_create(&key0).await.unwrap();
    h.session_manager
        .add_message(&key0, "user", "hello epoch 0")
        .await
        .unwrap();
    h.session_manager
        .add_message(&key0, "assistant", "hi back")
        .await
        .unwrap();

    // Close epoch 0 and create epoch 1
    h.session_manager.close_session(&key0, None).await.unwrap();
    let key1 = epoch_key("agent-retain", 1);
    h.session_manager.get_or_create(&key1).await.unwrap();

    // Old session messages still accessible
    let history = h
        .session_manager
        .get_history(&key0, None)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "hello epoch 0");
    assert_eq!(history[1].content, "hi back");
}

#[tokio::test]
async fn p1_07_new_epoch_starts_with_empty_history() {
    let h = SessionProbeHarness::new();

    // Epoch 0 with messages
    let key0 = epoch_key("agent-empty", 0);
    h.session_manager.get_or_create(&key0).await.unwrap();
    h.session_manager
        .add_message(&key0, "user", "old message")
        .await
        .unwrap();

    // Epoch 1 should start empty
    let key1 = epoch_key("agent-empty", 1);
    h.session_manager.get_or_create(&key1).await.unwrap();

    let history = h
        .session_manager
        .get_history(&key1, None)
        .await
        .unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn p1_08_multiple_epoch_cycles() {
    let h = SessionProbeHarness::new();

    let topics = [
        Some("Rust基础".to_string()),
        Some("异步编程".to_string()),
        None, // epoch 2 stays open
    ];

    for epoch in 0..3u32 {
        let key = epoch_key("agent-multi", epoch);
        h.session_manager.get_or_create(&key).await.unwrap();

        // Close epochs 0 and 1 with topics
        if epoch < 2 {
            h.session_manager
                .close_session(&key, topics[epoch as usize].clone())
                .await
                .unwrap();
        }
    }

    // Verify 3 sessions exist
    let sessions = h.list_sessions(Some("agent-multi")).await;
    assert_eq!(sessions.len(), 3);

    // Verify topics on closed sessions
    let topic0 = h
        .session_manager
        .get_session_topic(&epoch_key("agent-multi", 0))
        .await
        .unwrap();
    assert_eq!(topic0, Some("Rust基础".to_string()));

    let topic1 = h
        .session_manager
        .get_session_topic(&epoch_key("agent-multi", 1))
        .await
        .unwrap();
    assert_eq!(topic1, Some("异步编程".to_string()));

    let topic2 = h
        .session_manager
        .get_session_topic(&epoch_key("agent-multi", 2))
        .await
        .unwrap();
    assert!(topic2.is_none());
}
