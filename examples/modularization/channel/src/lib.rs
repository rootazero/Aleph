//! Standalone API-only Channel consumer.
//!
//! This example is deliberately transport-free: it models lifecycle and typed
//! contract behavior without a Core adapter, runtime, network, or platform SDK.

use aleph_slot_api::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    PairingData, SendOperation, SendResult,
};
use aleph_slot_api::{async_trait, json, Value};

pub struct ExampleChannel {
    info: ChannelInfo,
    state: ChannelState,
}

impl ExampleChannel {
    pub fn new() -> Self {
        Self {
            info: ChannelInfo {
                id: ChannelId::new("channel-1"),
                name: "External example channel".to_owned(),
                channel_type: "external-example".to_owned(),
                status: ChannelStatus::Disconnected,
                capabilities: ChannelCapabilities {
                    replies: true,
                    ..ChannelCapabilities::default()
                },
            },
            state: ChannelState::new(16),
        }
    }

    fn send_result(message_id: MessageId) -> ChannelResult<SendResult> {
        // `SendResult` contains chrono's private-to-this-consumer DateTime<Utc>
        // field. The API re-exports serde_json, so deserialize the real public
        // struct with its actual fields without adding a direct chrono dep.
        aleph_slot_api::serde_json::from_value(json!({
            "message_id": message_id.0,
            "timestamp": "1970-01-01T00:00:00Z"
        }))
        .map_err(|error| ChannelError::Internal(format!("invalid send result: {error}")))
    }
}

impl Default for ExampleChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Channel for ExampleChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.state.set_status(ChannelStatus::Connected).await;
        self.info.status = ChannelStatus::Connected;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.state.set_status(ChannelStatus::Disconnected).await;
        self.info.status = ChannelStatus::Disconnected;
        Ok(())
    }

    fn send(&self, message: OutboundMessage) -> SendOperation {
        let state = self.state.clone();
        let channel_id = self.info.id.clone();
        Box::pin(async move {
            if state.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(channel_id.0));
            }

            Self::send_result(MessageId::new(format!(
                "sent:{}",
                message.conversation_id.0
            )))
        })
    }
}

pub struct ExampleChannelFactory;

#[async_trait]
impl ChannelFactory for ExampleChannelFactory {
    fn channel_type(&self) -> &str {
        "external-example"
    }

    async fn create(&self, _config: Value) -> ChannelResult<Box<dyn Channel>> {
        Ok(Box::new(ExampleChannel::new()))
    }

    // `create_with_id` intentionally uses ChannelFactory's default body. The
    // API contract ignores the id and delegates to `create`.
}

fn require_unsupported<T>(result: ChannelResult<T>) -> ChannelResult<()> {
    match result {
        Err(ChannelError::UnsupportedFeature(_)) => Ok(()),
        Err(error) => Err(ChannelError::Internal(format!(
            "expected UnsupportedFeature, got {error}"
        ))),
        Ok(_) => Err(ChannelError::Internal(
            "unsupported operation unexpectedly succeeded".to_owned(),
        )),
    }
}

fn require_not_connected<T>(result: ChannelResult<T>) -> ChannelResult<()> {
    match result {
        Err(ChannelError::NotConnected(_)) => Ok(()),
        Err(error) => Err(ChannelError::Internal(format!(
            "expected NotConnected, got {error}"
        ))),
        Ok(_) => Err(ChannelError::Internal(
            "disconnected send unexpectedly succeeded".to_owned(),
        )),
    }
}

/// Exercise the lifecycle, message builder, factory delegation, and defaults.
///
/// The caller owns the executor; this API-only crate installs no runtime.
pub async fn round_trip() -> ChannelResult<SendResult> {
    let factory = ExampleChannelFactory;
    assert_eq!(factory.channel_type(), "external-example");

    let mut channel = factory
        .create_with_id("configured-instance", json!({}))
        .await?;
    assert_eq!(channel.info().id.0, "channel-1");
    assert_eq!(channel.state().status(), ChannelStatus::Disconnected);
    assert_eq!(channel.info().status, ChannelStatus::Disconnected);

    assert!(channel.approval_capability().is_none());
    assert!(channel.webhook_handler().is_none());
    assert!(channel.native_stream_handler().is_none());
    assert!(matches!(
        channel.get_pairing_data().await?,
        PairingData::None
    ));
    assert!(channel.list_active_pairing_codes().await?.is_empty());

    require_unsupported(
        channel
            .send_typing(&ConversationId::new("conversation-1"))
            .await,
    )?;
    require_unsupported(channel.mark_read(&MessageId::new("incoming-1")).await)?;
    require_unsupported(
        channel
            .react(
                &ConversationId::new("conversation-1"),
                &MessageId::new("incoming-1"),
                "like",
            )
            .await,
    )?;
    require_unsupported(channel.list_conversations("", 10).await)?;

    channel.start().await?;
    assert_eq!(channel.state().status(), ChannelStatus::Connected);
    assert_eq!(channel.info().status, ChannelStatus::Connected);

    let message = OutboundMessage::text("conversation-1", "hello").with_reply_to("incoming-1");
    assert_eq!(message.reply_to, Some(MessageId::new("incoming-1")));
    let result = channel.send(message).await?;
    assert_eq!(result.message_id, MessageId::new("sent:conversation-1"));

    channel.stop().await?;
    assert_eq!(channel.state().status(), ChannelStatus::Disconnected);
    assert_eq!(channel.info().status, ChannelStatus::Disconnected);
    require_not_connected(
        channel
            .send(OutboundMessage::text("conversation-1", "after stop"))
            .await,
    )?;

    Ok(result)
}
