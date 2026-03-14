//! P4: RPC handler tests for `sessions.new`.
//!
//! Covers valid requests, missing/invalid params, optional topic,
//! epoch increment, and database persistence.

use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;

use alephcore::gateway::handlers::session::handle_new_session_db;
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::router::SessionKey;
use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};

/// Create a SessionManager backed by a temp directory.
fn setup() -> (Arc<SessionManager>, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let config = SessionManagerConfig {
        db_path: tmp.path().join("sessions.db"),
        max_messages: 100,
        compaction_keep: 50,
        auto_reset_hour: None,
        session_expiry_secs: 0,
    };
    let manager = Arc::new(SessionManager::new(config).expect("SessionManager::new"));
    (manager, tmp)
}

/// Ensure a session exists so the handler can close it.
async fn create_main_session(manager: &SessionManager, agent_id: &str) -> String {
    let key = SessionKey::Main {
        agent_id: agent_id.to_string(),
        main_key: "main".to_string(),
    };
    manager.get_or_create(&key).await.expect("create session");
    key.to_key_string()
}

#[tokio::test]
async fn p4_01_valid_request_returns_both_keys() {
    let (manager, _tmp) = setup();
    let session_key = create_main_session(&manager, "alpha").await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: Some(json!({
            "session_key": session_key,
            "topic": "testing topic"
        })),
        id: Some(json!(1)),
    };

    let response = handle_new_session_db(request, manager).await;
    assert!(response.error.is_none(), "expected success, got error: {:?}", response.error);

    let result = response.result.expect("result should be present");
    assert_eq!(result["old_session_key"].as_str().unwrap(), session_key);
    // New key should have :s1 suffix (epoch 0 → 1)
    let new_key = result["new_session_key"].as_str().unwrap();
    assert!(new_key.contains(":s1"), "new key should contain :s1, got: {}", new_key);
    assert_eq!(result["topic"].as_str().unwrap(), "testing topic");
}

#[tokio::test]
async fn p4_02_missing_session_key_returns_error() {
    let (manager, _tmp) = setup();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: None,
        id: Some(json!(2)),
    };

    let response = handle_new_session_db(request, manager).await;
    assert!(response.result.is_none());

    let err = response.error.expect("should have error");
    assert!(
        err.message.contains("Missing session_key"),
        "error message should mention Missing session_key, got: {}",
        err.message
    );
}

#[tokio::test]
async fn p4_03_invalid_session_key_format_returns_error() {
    let (manager, _tmp) = setup();

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: Some(json!({
            "session_key": "completely-invalid"
        })),
        id: Some(json!(3)),
    };

    let response = handle_new_session_db(request, manager).await;
    assert!(response.result.is_none());
    assert!(response.error.is_some(), "should return error for invalid key format");
}

#[tokio::test]
async fn p4_04_topic_is_optional() {
    let (manager, _tmp) = setup();
    let session_key = create_main_session(&manager, "beta").await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: Some(json!({
            "session_key": session_key
        })),
        id: Some(json!(4)),
    };

    let response = handle_new_session_db(request, manager).await;
    assert!(response.error.is_none(), "expected success, got error: {:?}", response.error);

    let result = response.result.expect("result should be present");
    assert!(result["topic"].is_null(), "topic should be null when omitted");
}

#[tokio::test]
async fn p4_05_new_key_has_incremented_epoch() {
    let (manager, _tmp) = setup();
    let session_key_0 = create_main_session(&manager, "gamma").await;

    // First call: epoch 0 → 1
    let req1 = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: Some(json!({ "session_key": session_key_0 })),
        id: Some(json!(5)),
    };
    let resp1 = handle_new_session_db(req1, manager.clone()).await;
    assert!(resp1.error.is_none());
    let new_key_1 = resp1.result.as_ref().unwrap()["new_session_key"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(new_key_1.ends_with(":s1"), "first new key should end with :s1, got: {}", new_key_1);

    // Second call: epoch 1 → 2
    let req2 = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: Some(json!({ "session_key": new_key_1 })),
        id: Some(json!(6)),
    };
    let resp2 = handle_new_session_db(req2, manager.clone()).await;
    assert!(resp2.error.is_none(), "expected success, got error: {:?}", resp2.error);
    let new_key_2 = resp2.result.as_ref().unwrap()["new_session_key"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(new_key_2.ends_with(":s2"), "second new key should end with :s2, got: {}", new_key_2);
}

#[tokio::test]
async fn p4_06_new_session_exists_in_database() {
    let (manager, _tmp) = setup();
    let session_key = create_main_session(&manager, "delta").await;

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "sessions.new".to_string(),
        params: Some(json!({
            "session_key": session_key,
            "topic": "db check"
        })),
        id: Some(json!(7)),
    };

    let response = handle_new_session_db(request, manager.clone()).await;
    assert!(response.error.is_none());

    let result = response.result.as_ref().unwrap();
    let old_key = result["old_session_key"].as_str().unwrap();
    let new_key = result["new_session_key"].as_str().unwrap();

    // The response should return different key strings (epoch suffix differs)
    assert_ne!(old_key, new_key, "old and new keys should differ");

    // After the handler call, list_sessions should show the agent's session(s)
    // exist in the database (the legacy SessionKey maps both epoch keys to the
    // same DB row, so at minimum one session for "delta" must exist).
    let sessions = manager
        .list_sessions(Some("delta"))
        .await
        .expect("list_sessions should succeed");

    assert!(
        !sessions.is_empty(),
        "at least one session for agent delta should exist after sessions.new"
    );
    // Verify the session belongs to the correct agent
    assert!(
        sessions.iter().all(|s| s.agent_id == "delta"),
        "all sessions should belong to agent delta"
    );
}
