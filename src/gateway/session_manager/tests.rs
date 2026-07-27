#![allow(deprecated)]

use super::*;
use tempfile::tempdir;

fn test_config(path: PathBuf) -> SessionManagerConfig {
    SessionManagerConfig {
        db_path: path,
        max_messages: 10,
        compaction_keep: 5,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_session_creation() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    let meta = manager.get_or_create(&key).await.unwrap();

    assert_eq!(meta.agent_id, "test");
    assert_eq!(meta.session_type, "main");
    assert_eq!(meta.message_count, 0);
}

#[tokio::test]
async fn test_message_operations() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Add messages
    manager.add_message(&key, "user", "Hello").await.unwrap();
    manager
        .add_message(&key, "assistant", "Hi there!")
        .await
        .unwrap();

    let history = manager.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

#[tokio::test]
async fn test_get_history_before_cursor_filters_by_timestamp() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "one").await.unwrap();
    manager.add_message(&key, "assistant", "two").await.unwrap();
    manager.add_message(&key, "user", "three").await.unwrap();

    let now = chrono::Utc::now().timestamp();

    // No cursor → identical to plain get_history (all three).
    let all = manager.get_history_before(&key, None, None).await.unwrap();
    assert_eq!(all.len(), 3);

    // Cursor in the future → every message is strictly older, so all survive.
    let before_future = manager
        .get_history_before(&key, None, Some(now + 3600))
        .await
        .unwrap();
    assert_eq!(before_future.len(), 3);

    // Cursor far in the past → nothing is older than it.
    let before_past = manager
        .get_history_before(&key, None, Some(now - 3600))
        .await
        .unwrap();
    assert!(before_past.is_empty());

    // Limit still windows the cursor-filtered set (most-recent `limit`).
    let windowed = manager
        .get_history_before(&key, Some(2), Some(now + 3600))
        .await
        .unwrap();
    assert_eq!(windowed.len(), 2);
}

#[tokio::test]
async fn test_session_reset() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "Test").await.unwrap();

    assert!(manager.reset_session(&key).await.unwrap());

    let history = manager.get_history(&key, None).await.unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn test_compaction() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Add more messages than max_messages
    for i in 0..15 {
        manager
            .add_message(&key, "user", &format!("Message {}", i))
            .await
            .unwrap();
    }

    // Compaction should have happened automatically
    let history = manager.get_history(&key, None).await.unwrap();
    assert!(history.len() <= 10); // Should be at most max_messages after compaction
}

#[tokio::test]
async fn test_list_sessions() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    manager
        .get_or_create(&SessionKey::main("agent1"))
        .await
        .unwrap();
    manager
        .get_or_create(&SessionKey::main("agent2"))
        .await
        .unwrap();
    manager
        .get_or_create(&SessionKey::peer("agent1", "peer1"))
        .await
        .unwrap();

    let all = manager.list_sessions(None).await.unwrap();
    assert_eq!(all.len(), 3);

    let agent1_only = manager.list_sessions(Some("agent1")).await.unwrap();
    assert_eq!(agent1_only.len(), 2);
}

#[tokio::test]
async fn test_set_source_channel_records_and_surfaces_origin() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Fresh session: origin is the "unknown" sentinel → surfaced as None.
    let before = manager.list_sessions(None).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].origin_channel(), None);

    // Stamp the originating channel + conversation; both must be surfaced via
    // the read path.
    manager
        .set_source_channel(&key, "telegram", Some("chat-77"))
        .await
        .unwrap();
    let after = manager.list_sessions(None).await.unwrap();
    assert_eq!(
        after[0].origin_channel(),
        Some("telegram".to_string()),
        "first stamp must record the real origin"
    );
    assert_eq!(
        after[0].origin_conversation(),
        Some("chat-77".to_string()),
        "first stamp must capture the origin conversation id for reply fan-out"
    );

    // Idempotent: a later continuation from a different surface must NOT clobber
    // the recorded origin (this is what keeps multi-end continuity honest).
    manager
        .set_source_channel(&key, "gui:chat", None)
        .await
        .unwrap();
    let still = manager.list_sessions(None).await.unwrap();
    assert_eq!(
        still[0].origin_channel(),
        Some("telegram".to_string()),
        "second stamp must not clobber the existing origin"
    );
    assert_eq!(
        still[0].origin_conversation(),
        Some("chat-77".to_string()),
        "second stamp must not clobber the origin conversation either"
    );
}

#[tokio::test]
async fn test_set_source_channel_skips_empty_and_unknown_sentinel() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Empty / sentinel inputs are no-ops: origin stays unset (None).
    manager.set_source_channel(&key, "   ", None).await.unwrap();
    manager
        .set_source_channel(&key, "unknown", Some("x"))
        .await
        .unwrap();
    let sessions = manager.list_sessions(None).await.unwrap();
    assert_eq!(sessions[0].origin_channel(), None);
    assert_eq!(sessions[0].origin_conversation(), None);
}

#[test]
fn test_session_identity_meta_default() {
    let meta = SessionIdentityMeta::default();
    assert_eq!(meta.role, Role::Owner);
    assert_eq!(meta.identity_id, "owner");
    assert!(meta.scope.is_none());
    assert_eq!(meta.source_channel, "unknown");
}

#[test]
fn test_session_identity_meta_owner_factory() {
    let meta = SessionIdentityMeta::owner("cli");
    assert_eq!(meta.role, Role::Owner);
    assert_eq!(meta.identity_id, "owner");
    assert!(meta.scope.is_none());
    assert_eq!(meta.source_channel, "cli");
}

#[test]
fn test_session_identity_meta_guest_factory() {
    let scope = GuestScope {
        allowed_tools: vec!["translate".to_string()],
        expires_at: Some(2000),
        display_name: Some("Test Guest".to_string()),
    };

    let meta = SessionIdentityMeta::guest("guest-123", scope.clone(), "telegram");
    assert_eq!(meta.role, Role::Guest);
    assert_eq!(meta.identity_id, "guest-123");
    assert_eq!(meta.scope, Some(scope));
    assert_eq!(meta.source_channel, "telegram");
}

#[test]
fn test_session_identity_meta_json_roundtrip() {
    let scope = GuestScope {
        allowed_tools: vec!["tool1".to_string(), "tool2".to_string()],
        expires_at: None,
        display_name: None,
    };

    let meta = SessionIdentityMeta::guest("guest-456", scope, "web");
    let json = meta.to_json_string().unwrap();
    let parsed = SessionIdentityMeta::from_json_str(Some(&json));

    assert_eq!(parsed.role, meta.role);
    assert_eq!(parsed.identity_id, meta.identity_id);
    assert_eq!(parsed.scope, meta.scope);
    assert_eq!(parsed.source_channel, meta.source_channel);
}

#[test]
fn test_session_identity_meta_from_null_json() {
    let meta = SessionIdentityMeta::from_json_str(None);
    assert_eq!(meta.role, Role::Owner); // Default
    assert_eq!(meta.identity_id, "owner");
}

#[test]
fn test_session_identity_meta_from_invalid_json() {
    let meta = SessionIdentityMeta::from_json_str(Some("{invalid json}"));
    assert_eq!(meta.role, Role::Owner); // Fallback to default
}

#[tokio::test]
async fn test_tool_fields_persist_through_append_message_and_get_history() {
    use crate::gateway::session_store::types::MessageRecord;
    use crate::gateway::session_store::SessionStore;

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Write a tool-result message through the SessionStore trait method — the
    // exact forwarding path (append_message → add_message_full) whose
    // correctness matters: it must forward tool_call_id/tool_name, not None.
    let msg = MessageRecord {
        id: "1".into(),
        role: "tool".into(),
        content: r#"{"result":"ok"}"#.into(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: Some("call_abc123".into()),
        tool_name: Some("bash_exec".into()),
    };
    <SessionManager as SessionStore>::append_message(&manager, &key, msg)
        .await
        .unwrap();

    let history = manager.get_history(&key, None).await.unwrap();
    assert_eq!(history.len(), 1);
    let msg: &MessageRecord = &history[0];
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id.as_deref(), Some("call_abc123"));
    assert_eq!(msg.tool_name.as_deref(), Some("bash_exec"));
}

#[tokio::test]
async fn test_close_session_with_topic() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();
    manager.add_message(&key, "user", "Hello").await.unwrap();

    manager
        .close_session(&key, Some("测试对话".to_string()))
        .await
        .unwrap();

    // Verify topic can be retrieved
    let topic = manager.get_session_topic(&key).await.unwrap();
    assert_eq!(topic, Some("测试对话".to_string()));
}

#[tokio::test]
async fn legacy_session_history_readable_without_events() {
    use crate::gateway::session_store::types::MessageRecord;
    use crate::gateway::session_store::SessionStore;

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    // Write two messages directly (legacy session — no session_events).
    // This simulates a session created before the MessageProjector flip.
    let user_msg = MessageRecord {
        id: "msg_1".into(),
        role: "user".into(),
        content: "Hello, assistant!".into(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: None,
        tool_name: None,
    };
    <SessionManager as SessionStore>::append_message(&manager, &key, user_msg)
        .await
        .unwrap();

    let assistant_msg = MessageRecord {
        id: "msg_2".into(),
        role: "assistant".into(),
        content: "Hi there!".into(),
        timestamp: chrono::Utc::now().timestamp(),
        metadata: None,
        input_tokens: 0,
        output_tokens: 0,
        tool_call_id: None,
        tool_name: None,
    };
    <SessionManager as SessionStore>::append_message(&manager, &key, assistant_msg)
        .await
        .unwrap();

    // Assert both messages are readable through the read surface (messages table).
    let history = <SessionManager as SessionStore>::get_history(&manager, &key, None)
        .await
        .unwrap();
    assert_eq!(
        history.len(),
        2,
        "legacy session should return both messages"
    );
    assert_eq!(history[0].role, "user", "first message should be user role");
    assert_eq!(
        history[0].content, "Hello, assistant!",
        "first message content mismatch"
    );
    assert_eq!(
        history[1].role, "assistant",
        "second message should be assistant role"
    );
    assert_eq!(
        history[1].content, "Hi there!",
        "second message content mismatch"
    );
}

#[tokio::test]
async fn test_close_session_without_topic() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    let key = SessionKey::main("test");
    manager.get_or_create(&key).await.unwrap();

    manager.close_session(&key, None).await.unwrap();

    let topic = manager.get_session_topic(&key).await.unwrap();
    assert!(topic.is_none());
}

#[tokio::test]
async fn test_get_current_epoch() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    // Create epoch 0
    let key0 = SessionKey::main("test");
    manager.get_or_create(&key0).await.unwrap();
    let epoch = manager.get_current_epoch("agent:test:main").await.unwrap();
    assert_eq!(epoch, 0);
}

#[test]
fn test_session_identity_meta_to_identity_context_owner() {
    let meta = SessionIdentityMeta::owner("cli");
    let ctx = meta.to_identity_context("session:main".to_string());

    assert_eq!(ctx.session_key, "session:main");
    assert_eq!(ctx.role, Role::Owner);
    assert_eq!(ctx.identity_id, "owner");
    assert_eq!(ctx.source_channel, "cli");
    assert!(ctx.scope.is_none());
}

#[test]
fn test_session_identity_meta_to_identity_context_guest() {
    let scope = GuestScope {
        allowed_tools: vec!["translate".to_string()],
        expires_at: Some(3000),
        display_name: Some("Guest".to_string()),
    };

    let meta = SessionIdentityMeta::guest("guest-789", scope.clone(), "telegram");
    let ctx = meta.to_identity_context("session:guest".to_string());

    assert_eq!(ctx.session_key, "session:guest");
    assert_eq!(ctx.role, Role::Guest);
    assert_eq!(ctx.identity_id, "guest-789");
    assert_eq!(ctx.source_channel, "telegram");
    assert_eq!(ctx.scope, Some(scope));
}

#[tokio::test]
async fn test_get_total_tokens_none_then_accumulates() {
    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();
    let key = SessionKey::main("tok");

    // No row yet → None.
    assert_eq!(manager.get_total_tokens(&key).await.unwrap(), None);

    manager.get_or_create(&key).await.unwrap();
    // Fresh row → 0.
    assert_eq!(manager.get_total_tokens(&key).await.unwrap(), Some(0));

    manager
        .update_session_usage(&key, 100, 40, 0.25, None, None)
        .await
        .unwrap();
    manager
        .update_session_usage(&key, 10, 5, 0.05, None, None)
        .await
        .unwrap();
    // Cumulative input+output across both turns: 140 + 15 = 155.
    assert_eq!(manager.get_total_tokens(&key).await.unwrap(), Some(155));

    // …and the cost accumulates on the same row. `estimated_cost_usd` had no
    // column and no writer until now, yet the `sessions` tool and the Panel both
    // reported it to the user as this session's spend — permanently $0.00.
    let sessions = manager.list_sessions(None).await.unwrap();
    let meta = sessions
        .iter()
        .find(|m| m.key == key.to_key_string())
        .expect("session row");
    assert!(
        (meta.estimated_cost_usd - 0.30).abs() < 1e-9,
        "expected 0.25 + 0.05 = 0.30, got {}",
        meta.estimated_cost_usd
    );
}

#[tokio::test]
async fn goal_tree_budget_sums_own_plus_member_deltas_only() {
    use crate::gateway::goal_budget::tree_tokens;
    use crate::gateway::session_store::SessionStore;
    use crate::goal::types::{BudgetMember, Goal};
    use std::sync::Arc;

    let temp = tempdir().unwrap();
    let config = test_config(temp.path().join("test.db"));
    let manager = SessionManager::new(config).unwrap();

    // Owner session: seed 100+40 = 140 cumulative tokens.
    let own_key = SessionKey::main("leader");
    manager.get_or_create(&own_key).await.unwrap();
    manager
        .update_session_usage(&own_key, 100, 40, 0.0, None, None)
        .await
        .unwrap();

    // Enrolled member (a delegated task session): seed 200+60 = 260; joined at 60.
    let member_key = SessionKey::task("worker", "team", "task-1");
    manager.get_or_create(&member_key).await.unwrap();
    manager
        .update_session_usage(&member_key, 200, 60, 0.0, None, None)
        .await
        .unwrap();

    // An UNENROLLED session (stands in for an in-process subagent — never a budget member).
    let stray_key = SessionKey::task("worker", "team", "stray");
    manager.get_or_create(&stray_key).await.unwrap();
    manager
        .update_session_usage(&stray_key, 9_999, 9_999, 0.0, None, None)
        .await
        .unwrap();

    // Goal owned by own_key; one enrolled member with tokens_at_join = 60.
    let mut goal = Goal::new(&own_key.to_key_string(), "obj", 0, 0);
    goal.token_budget = Some(10_000);
    goal.budget_members = vec![BudgetMember {
        session_id: member_key.to_key_string(),
        tokens_at_join: 60,
    }];

    let store: Arc<dyn SessionStore> = Arc::new(manager);
    let total = tree_tokens(&store, &goal, &own_key)
        .await
        .expect("own total readable");

    // own(140) + member_delta(260 - 60 = 200) = 340. The stray 19_998 is absent.
    assert_eq!(
        total, 340,
        "only own row + enrolled member delta count; unenrolled spend is invisible"
    );
}
