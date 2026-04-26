use alephcore::gateway::channel::{ChannelId, MessageId};
use alephcore::gateway::interfaces::mattermost::message_ops::MattermostMessageOps;
use alephcore::gateway::interfaces::mattermost::MattermostConfig;

#[test]
fn test_users_me_fixture() {
    let fixture = include_str!("fixtures/mattermost/users_me.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();

    assert_eq!(data["id"].as_str().unwrap(), "user-bot-123");
    assert_eq!(data["username"].as_str().unwrap(), "aleph-bot");
    assert_eq!(data["email"].as_str().unwrap(), "bot@example.com");
}

#[test]
fn test_create_post_fixture() {
    let fixture = include_str!("fixtures/mattermost/create_post.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();

    assert_eq!(data["id"].as_str().unwrap(), "post-abc-123");
    assert_eq!(data["channel_id"].as_str().unwrap(), "ch-789");
    assert_eq!(data["message"].as_str().unwrap(), "Hello from Mattermost!");
    assert_eq!(data["user_id"].as_str().unwrap(), "user-bot-123");
    assert_eq!(data["create_at"].as_i64().unwrap(), 1700000000000);
}

#[test]
fn test_posted_event_fixture() {
    let fixture = include_str!("fixtures/mattermost/posted_event.json");
    let event: serde_json::Value = serde_json::from_str(fixture).unwrap();

    assert_eq!(event["event"].as_str().unwrap(), "posted");

    let post_str = event["data"]["post"].as_str().unwrap();
    let post: serde_json::Value = serde_json::from_str(post_str).unwrap();

    assert_eq!(post["id"].as_str().unwrap(), "post-user-456");
    assert_eq!(post["channel_id"].as_str().unwrap(), "ch-789");
    assert_eq!(post["message"].as_str().unwrap(), "Hello bot!");
    assert_eq!(post["user_id"].as_str().unwrap(), "user-456");

    assert_eq!(event["data"]["channel_type"].as_str().unwrap(), "O");
    assert_eq!(event["data"]["sender_name"].as_str().unwrap(), "alice");
}

#[test]
fn test_convert_posted_event_from_fixture() {
    let fixture = include_str!("fixtures/mattermost/posted_event.json");
    let event: serde_json::Value = serde_json::from_str(fixture).unwrap();

    let channel_id = ChannelId::new("mattermost");
    let config = MattermostConfig {
        allowed_channels: vec!["ch-789".to_string()],
        ..Default::default()
    };

    let msg = MattermostMessageOps::convert_posted_event(
        &event,
        &channel_id,
        "bot-123",
        &config,
    )
    .unwrap();

    assert_eq!(msg.channel_id.as_str(), "mattermost");
    assert_eq!(msg.conversation_id.as_str(), "ch-789");
    assert_eq!(msg.sender_id.as_str(), "user-456");
    assert_eq!(msg.sender_name.as_deref(), Some("alice"));
    assert_eq!(msg.text, "Hello bot!");
    assert!(msg.is_group);
    assert!(msg.reply_to.is_none());
    assert_eq!(msg.id.as_str(), "post-user-456");
}

#[test]
fn test_convert_skips_own_message_from_fixture() {
    let mut event: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mattermost/posted_event.json")).unwrap();

    let post_str = event["data"]["post"].as_str().unwrap();
    let mut post: serde_json::Value = serde_json::from_str(post_str).unwrap();
    post["user_id"] = serde_json::Value::String("bot-123".to_string());
    event["data"]["post"] =
        serde_json::Value::String(serde_json::to_string(&post).unwrap());

    let channel_id = ChannelId::new("mattermost");
    let config = MattermostConfig::default();

    let msg = MattermostMessageOps::convert_posted_event(
        &event,
        &channel_id,
        "bot-123",
        &config,
    );
    assert!(msg.is_none());
}

#[test]
fn test_convert_skips_filtered_channel_from_fixture() {
    let fixture = include_str!("fixtures/mattermost/posted_event.json");
    let event: serde_json::Value = serde_json::from_str(fixture).unwrap();

    let channel_id = ChannelId::new("mattermost");
    let config = MattermostConfig {
        allowed_channels: vec!["ch-111".to_string()],
        ..Default::default()
    };

    let msg = MattermostMessageOps::convert_posted_event(
        &event,
        &channel_id,
        "bot-123",
        &config,
    );
    assert!(msg.is_none());
}

#[test]
fn test_convert_dm_channel_type_from_fixture() {
    let mut event: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mattermost/posted_event.json")).unwrap();
    event["data"]["channel_type"] = serde_json::Value::String("D".to_string());

    let channel_id = ChannelId::new("mattermost");
    let config = MattermostConfig::default();

    let msg = MattermostMessageOps::convert_posted_event(
        &event,
        &channel_id,
        "bot-123",
        &config,
    )
    .unwrap();

    assert!(!msg.is_group);
}

#[test]
fn test_convert_threaded_reply_from_fixture() {
    let mut event: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mattermost/posted_event.json")).unwrap();

    let post_str = event["data"]["post"].as_str().unwrap();
    let mut post: serde_json::Value = serde_json::from_str(post_str).unwrap();
    post["root_id"] = serde_json::Value::String("post-root-456".to_string());
    event["data"]["post"] =
        serde_json::Value::String(serde_json::to_string(&post).unwrap());

    let channel_id = ChannelId::new("mattermost");
    let config = MattermostConfig::default();

    let msg = MattermostMessageOps::convert_posted_event(
        &event,
        &channel_id,
        "bot-123",
        &config,
    )
    .unwrap();

    assert_eq!(
        msg.reply_to.as_ref().unwrap().as_str(),
        "post-root-456"
    );
}
