mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::matrix::{MatrixChannel, MatrixConfig};
use common::channel_contract::test_channel_properties;

fn test_matrix_config() -> MatrixConfig {
    MatrixConfig {
        homeserver_url: "https://matrix.org".to_string(),
        access_token: "test_token".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_matrix_properties() {
    let channel = MatrixChannel::new("test-matrix", test_matrix_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "matrix");
    assert!(channel.capabilities().typing_indicator);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().attachments);
    assert!(channel.capabilities().rich_text);
    assert!(channel.capabilities().editing);
    assert!(channel.capabilities().deletion);
    // The adapter implements no read-receipt method, so the bit stays false.
    assert!(!channel.capabilities().read_receipts);
    assert_eq!(channel.capabilities().max_message_length, 65535);
    assert_eq!(
        channel.capabilities().max_attachment_size,
        100 * 1024 * 1024
    );
}

#[test]
fn test_matrix_for_test_constructor() {
    let config = test_matrix_config();
    let channel = MatrixChannel::for_test("test-matrix", config);

    assert_eq!(channel.info().id.as_str(), "test-matrix");
    assert_eq!(channel.channel_type(), "matrix");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Disconnected
    );
}

#[tokio::test]
async fn test_matrix_start_in_test_mode() {
    let mut channel = MatrixChannel::for_test("test-matrix", test_matrix_config());

    let result = channel.start().await;
    assert!(result.is_ok(), "start() should succeed in test mode");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Connected
    );
}
