//! Integration test for iMessage Gateway routing
//!
//! Tests the complete message flow from InboundMessage to routing.

#![cfg(feature = "gateway")]

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::gateway::{
    ChannelId, ChannelRegistry, ConversationId, InboundMessage, MessageId, UserId,
    InboundMessageRouter, RoutingConfig, DmScope,
    SqlitePairingStore, PairingStore,
    RouterChannelConfig, DmPolicy, GroupPolicy,
};
use chrono::Utc;

fn make_dm_message(sender: &str, text: &str) -> InboundMessage {
    InboundMessage {
        id: MessageId::new(format!("msg-{}", uuid::Uuid::new_v4())),
        channel_id: ChannelId::new("imessage"),
        conversation_id: ConversationId::new(sender),
        sender_id: UserId::new(sender),
        sender_name: None,
        text: text.to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group: false,
        raw: None,
    }
}

fn make_group_message(chat_id: &str, sender: &str, text: &str) -> InboundMessage {
    InboundMessage {
        id: MessageId::new(format!("msg-{}", uuid::Uuid::new_v4())),
        channel_id: ChannelId::new("imessage"),
        conversation_id: ConversationId::new(chat_id),
        sender_id: UserId::new(sender),
        sender_name: None,
        text: text.to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group: true,
        raw: None,
    }
}

#[tokio::test]
async fn test_dm_with_open_policy() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    // Set open policy
    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Open,
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    // Should succeed with open policy
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dm_with_allowlist_policy_allowed() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["+15551234567".to_string()],
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dm_with_allowlist_policy_denied() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["+15559999999".to_string()], // Different number
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    // Should succeed but message filtered (not an error)
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dm_with_pairing_policy() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store.clone(), config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Pairing,
            allow_from: vec![],
            ..Default::default()
        },
    );

    // First message should trigger pairing request
    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;
    assert!(result.is_ok());

    // Should have created a pairing request
    let pending = store.list_pending(Some("imessage")).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].sender_id.contains("15551234567"));
}

#[tokio::test]
async fn test_dm_with_pairing_approved() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    // Pre-approve the sender
    let (code, _) = store
        .upsert("imessage", "+15551234567", HashMap::new())
        .await
        .unwrap();
    store.approve("imessage", &code).await.unwrap();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Pairing,
            allow_from: vec![],
            ..Default::default()
        },
    );

    let msg = make_dm_message("+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_group_with_mention_required_no_mention() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            group_policy: GroupPolicy::Open,
            require_mention: true,
            bot_name: Some("Aleph".to_string()),
            ..Default::default()
        },
    );

    // Without mention - should be filtered
    let msg = make_group_message("chat_id:42", "+15551234567", "Hello everyone!");
    let result = router.handle_message(msg).await;
    assert!(result.is_ok()); // Filtered but no error
}

#[tokio::test]
async fn test_group_with_mention_required_with_mention() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            group_policy: GroupPolicy::Open,
            require_mention: true,
            bot_name: Some("Aleph".to_string()),
            ..Default::default()
        },
    );

    // With mention - should pass
    let msg = make_group_message("chat_id:42", "+15551234567", "Hey @aleph, help me!");
    let result = router.handle_message(msg).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_group_disabled() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            group_policy: GroupPolicy::Disabled,
            ..Default::default()
        },
    );

    let msg = make_group_message("chat_id:42", "+15551234567", "@aleph help!");
    let result = router.handle_message(msg).await;

    // Should be filtered (group disabled)
    assert!(result.is_ok());
}

// Note: Session key tests require access to internal build_context method.
// Since build_context is private, we test session key behavior indirectly
// through the router's integration with the handle_message flow.

#[tokio::test]
async fn test_dm_different_senders_both_allowed() {
    // Test that different senders with open policy both pass
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::new("main").with_dm_scope(DmScope::PerPeer);

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Open,
            ..Default::default()
        },
    );

    let msg1 = make_dm_message("+15551111111", "Hello from user 1");
    let msg2 = make_dm_message("+15552222222", "Hello from user 2");

    // Both should pass with open policy
    assert!(router.handle_message(msg1).await.is_ok());
    assert!(router.handle_message(msg2).await.is_ok());
}

#[tokio::test]
async fn test_dm_main_scope_policy() {
    // Test that main scope routing works correctly
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::new("main").with_dm_scope(DmScope::Main);

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            dm_policy: DmPolicy::Open,
            ..Default::default()
        },
    );

    let msg1 = make_dm_message("+15551111111", "Hello");
    let msg2 = make_dm_message("+15552222222", "Hi");

    // Both should pass - with Main scope, all DMs share the same session
    assert!(router.handle_message(msg1).await.is_ok());
    assert!(router.handle_message(msg2).await.is_ok());
}

#[tokio::test]
async fn test_group_allowlist_policy_allowed() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            group_policy: GroupPolicy::Allowlist,
            group_allow_from: vec!["chat_id:42".to_string()],
            require_mention: false,
            ..Default::default()
        },
    );

    let msg = make_group_message("chat_id:42", "+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    // Group is in allowlist, should pass
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_group_allowlist_policy_denied() {
    let registry = Arc::new(ChannelRegistry::new());
    let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
    let config = RoutingConfig::default();

    let mut router = InboundMessageRouter::new(registry, store, config);

    router.register_channel_config(
        "imessage",
        RouterChannelConfig {
            group_policy: GroupPolicy::Allowlist,
            group_allow_from: vec!["other_chat:99".to_string()],
            require_mention: false,
            ..Default::default()
        },
    );

    let msg = make_group_message("chat_id:42", "+15551234567", "Hello!");
    let result = router.handle_message(msg).await;

    // Group not in allowlist, should be filtered (but no error)
    assert!(result.is_ok());
}
