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
