mod common;

use alephcore::gateway::channel::{Channel, ConversationId, OutboundMessage};
use alephcore::gateway::interfaces::email::{EmailChannel, EmailConfig};

fn test_email_config() -> EmailConfig {
    EmailConfig {
        imap_host: "imap.test.com".to_string(),
        smtp_host: "smtp.test.com".to_string(),
        username: "test@test.com".to_string(),
        password: "test-pass".to_string(),
        from_address: "aleph@test.com".to_string(),
        imap_port: 993,
        smtp_port: 587,
        use_tls: true,
        poll_interval_secs: 60,
        folders: vec!["INBOX".to_string()],
        allowed_senders: vec![],
    }
}

#[tokio::test]
async fn test_email_send_returns_result_in_test_mode() {
    let channel = EmailChannel::for_test("test-email", test_email_config());

    let result = channel
        .send(OutboundMessage::text("user@example.com", "Test body"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(
        send_result.message_id.as_str().starts_with("email-"),
        "message_id should start with 'email-' in test mode"
    );
}

#[tokio::test]
async fn test_email_send_with_subject_metadata() {
    let channel = EmailChannel::for_test("test-email", test_email_config());

    let mut msg = OutboundMessage::text("user@example.com", "Test body");
    msg.metadata
        .insert("subject".to_string(), "Custom Subject".to_string());

    let result = channel.send(msg).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_email_send_typing_returns_unsupported() {
    let channel = EmailChannel::for_test("test-email", test_email_config());

    let result = channel.send_typing(&ConversationId::new("user@example.com")).await;
    assert!(
        matches!(result, Err(alephcore::gateway::channel::ChannelError::UnsupportedFeature(_))),
        "Email should not support typing indicators"
    );
}

#[tokio::test]
async fn test_email_react_returns_unsupported() {
    let channel = EmailChannel::for_test("test-email", test_email_config());

    let result = channel
        .react(
            &ConversationId::new("user@example.com"),
            &alephcore::gateway::channel::MessageId::new("msg123"),
            "👍",
        )
        .await;
    assert!(
        matches!(result, Err(alephcore::gateway::channel::ChannelError::UnsupportedFeature(_))),
        "Email should not support reactions"
    );
}
