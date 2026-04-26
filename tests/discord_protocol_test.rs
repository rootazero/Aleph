use alephcore::gateway::channel::{Channel, ConversationId, MessageId, OutboundMessage};
use alephcore::gateway::interfaces::discord::DiscordChannel;

fn test_discord_config() -> alephcore::gateway::interfaces::discord::DiscordConfig {
    alephcore::gateway::interfaces::discord::DiscordConfig {
        bot_token: "test_token_that_is_long_enough_to_pass_validation_check".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_discord_send_message_in_test_mode() {
    let mut channel = DiscordChannel::for_test("test-discord", test_discord_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("123456", "Hello Discord!"))
        .await;

    assert!(result.is_ok(), "send() should succeed in test mode");
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "discord-test-msg-id");
}

#[tokio::test]
async fn test_discord_send_typing_in_test_mode() {
    let mut channel = DiscordChannel::for_test("test-discord", test_discord_config());
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&ConversationId::new("123456"))
        .await;

    assert!(result.is_ok(), "send_typing() should succeed in test mode");
}

#[tokio::test]
async fn test_discord_edit_in_test_mode() {
    let mut channel = DiscordChannel::for_test("test-discord", test_discord_config());
    channel.start().await.unwrap();

    let result = channel
        .edit(
            &ConversationId::new("123456"),
            &MessageId::new("789".to_string()),
            "Edited text",
        )
        .await;

    assert!(result.is_ok(), "edit() should succeed in test mode");
}

#[tokio::test]
async fn test_discord_delete_in_test_mode() {
    let mut channel = DiscordChannel::for_test("test-discord", test_discord_config());
    channel.start().await.unwrap();

    let result = channel
        .delete(
            &ConversationId::new("123456"),
            &MessageId::new("789".to_string()),
        )
        .await;

    assert!(result.is_ok(), "delete() should succeed in test mode");
}

#[tokio::test]
async fn test_discord_react_in_test_mode() {
    let mut channel = DiscordChannel::for_test("test-discord", test_discord_config());
    channel.start().await.unwrap();

    let result = channel
        .react(
            &ConversationId::new("123456"),
            &MessageId::new("789".to_string()),
            "👍",
        )
        .await;

    assert!(result.is_ok(), "react() should succeed in test mode");
}

#[tokio::test]
async fn test_discord_send_not_started_fails() {
    let channel = DiscordChannel::for_test("test-discord", test_discord_config());

    let result = channel
        .send(OutboundMessage::text("123456", "Hello"))
        .await;

    assert!(
        matches!(result, Err(alephcore::gateway::channel::ChannelError::NotConnected(_))),
        "send() should fail with NotConnected when not started"
    );
}
