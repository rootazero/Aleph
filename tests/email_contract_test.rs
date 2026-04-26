mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::email::{EmailChannel, EmailConfig};
use common::channel_contract::test_channel_properties;

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

#[test]
fn test_email_properties() {
    let channel = EmailChannel::new("test-email", test_email_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "email");
    assert!(!channel.capabilities().typing_indicator);
    assert!(!channel.capabilities().reactions);
    assert!(!channel.capabilities().editing);
    assert!(channel.capabilities().rich_text);
    assert!(channel.capabilities().attachments);
    assert_eq!(channel.capabilities().max_message_length, 1_048_576);
}

#[tokio::test]
async fn test_email_send_in_test_mode() {
    let channel = EmailChannel::for_test("test-email", test_email_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user@example.com",
            "Hello via email",
        ))
        .await;

    assert!(result.is_ok(), "send() should succeed in test mode");
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());
}
