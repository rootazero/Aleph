//! `SessionStore::truncate_messages` happy-path + boundary coverage.
//!
//! Drives the SQLite-backed `SessionManager` through the public `SessionStore`
//! trait — this is the path the new `session.truncate` RPC and the TUI `/undo`
//! command consume. Boundary cases mirror those called out in the design spec
//! (docs/superpowers/specs/2026-05-21-repl-agent-control-panel-design.md §6).

use alephcore::gateway::router::SessionKey;
use alephcore::gateway::session_store::types::MessageRecord;
use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::{SessionManager, SessionManagerConfig};

fn make_manager() -> (SessionManager, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("sessions.db");
    let sm = SessionManager::new(SessionManagerConfig {
        db_path,
        ..Default::default()
    })
    .expect("SessionManager");
    (sm, tmp)
}

fn record(role: &str, content: &str, input_tokens: i64, output_tokens: i64) -> MessageRecord {
    MessageRecord {
        id: String::new(),
        role: role.to_string(),
        content: content.to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens,
        output_tokens,
        model: None,
        model_provider: None,
    }
}

async fn seed(sm: &SessionManager, key: &SessionKey, n: usize) {
    sm.get_or_create(key).await.unwrap();
    for i in 0..n {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let r = record(role, &format!("msg-{i}"), 10, 5);
        <SessionManager as SessionStore>::append_message(sm, key, r)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn truncate_drops_tail_and_keeps_head() {
    let (sm, _tmp) = make_manager();
    let key = SessionKey::ephemeral("trunc-tail");
    seed(&sm, &key, 6).await;

    let result = sm.truncate_messages(&key, 4).await.unwrap();
    assert_eq!(result.messages_removed, 2, "2 tail messages should be dropped");
    // Each dropped message contributes input+output tokens = 15
    assert_eq!(result.tokens_removed_estimate, 30);

    let history = sm.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].content, "msg-0");
    assert_eq!(history[3].content, "msg-3");
}

#[tokio::test]
async fn truncate_noop_when_keep_count_exceeds_total() {
    let (sm, _tmp) = make_manager();
    let key = SessionKey::ephemeral("trunc-noop-over");
    seed(&sm, &key, 3).await;

    let result = sm.truncate_messages(&key, 999).await.unwrap();
    assert_eq!(result.messages_removed, 0);
    assert_eq!(result.tokens_removed_estimate, 0);

    let history = sm.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 3);
}

#[tokio::test]
async fn truncate_noop_when_keep_count_equals_total() {
    let (sm, _tmp) = make_manager();
    let key = SessionKey::ephemeral("trunc-noop-eq");
    seed(&sm, &key, 5).await;

    let result = sm.truncate_messages(&key, 5).await.unwrap();
    assert_eq!(result.messages_removed, 0);
    assert_eq!(result.tokens_removed_estimate, 0);

    let history = sm.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 5);
}

#[tokio::test]
async fn truncate_to_zero_deletes_all_messages() {
    let (sm, _tmp) = make_manager();
    let key = SessionKey::ephemeral("trunc-zero");
    seed(&sm, &key, 4).await;

    let result = sm.truncate_messages(&key, 0).await.unwrap();
    assert_eq!(result.messages_removed, 4);
    assert_eq!(result.tokens_removed_estimate, 60); // 4 * 15

    let history = sm.get_history(&key, None).await.unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn truncate_undo_drops_last_user_assistant_pair() {
    // Simulates /undo: drop the last user+assistant pair from a 10-message session.
    let (sm, _tmp) = make_manager();
    let key = SessionKey::ephemeral("trunc-undo");
    seed(&sm, &key, 10).await;

    let result = sm.truncate_messages(&key, 8).await.unwrap();
    assert_eq!(result.messages_removed, 2);

    let history = sm.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 8);
    // Original interleave: msg-0..msg-9 with user/assistant alternation
    assert_eq!(history.last().unwrap().content, "msg-7");
}

#[tokio::test]
async fn truncate_metadata_message_count_stays_consistent() {
    let (sm, _tmp) = make_manager();
    let key = SessionKey::ephemeral("trunc-meta");
    seed(&sm, &key, 6).await;

    sm.truncate_messages(&key, 3).await.unwrap();

    let meta = sm.get_metadata(&key).await.unwrap().expect("metadata");
    assert_eq!(meta.message_count, 3);
}
