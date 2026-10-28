//! Configurable channel lifecycle seam; a saved config does not attach a Relay.

use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, OutboundDeliveryPolicy, OutboundMessage,
    SendOperation,
};

use super::config::VoiceCallConfig;

const RELAY_UNAVAILABLE: &str =
    "Voice call Relay transport is not attached (configured but unavailable)";

/// Makes voice-call configuration reachable through the existing channel RPCs.
/// The offline call reducer and injected per-call Relay seam remain separate:
/// neither is evidence that a configurable channel has a live connection.
pub struct VoiceCallChannel {
    info: ChannelInfo,
    config: VoiceCallConfig,
    channel_state: ChannelState,
}

impl VoiceCallChannel {
    pub fn new(id: impl Into<String>, config: VoiceCallConfig) -> Self {
        Self {
            info: ChannelInfo {
                id: ChannelId::new(id),
                name: "Phone Call".into(),
                channel_type: "voice_call".into(),
                status: ChannelStatus::Disconnected,
                // PSTN media is handled by the Relay, not by attachment APIs.
                capabilities: ChannelCapabilities::default(),
            },
            config,
            channel_state: ChannelState::new(100),
        }
    }

    pub fn config(&self) -> &VoiceCallConfig {
        &self.config
    }

    async fn record_start_failure(&mut self, reason: &str) {
        self.info.status = ChannelStatus::Error;
        self.channel_state.set_status(ChannelStatus::Error).await;
        self.channel_state
            .record_failure(Some(reason.to_owned()))
            .await;
    }
}

#[async_trait]
impl Channel for VoiceCallChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    /// Call-scoped output must not be retried or persisted for later replay.
    fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
        OutboundDeliveryPolicy::ExplicitOnly
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if let Err(reason) = self.config.validate() {
            self.record_start_failure(&reason).await;
            return Err(ChannelError::ConfigError(reason));
        }
        self.record_start_failure(RELAY_UNAVAILABLE).await;
        Err(ChannelError::NotConnected(RELAY_UNAVAILABLE.into()))
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.info.status = ChannelStatus::Disconnected;
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    fn send(&self, _message: OutboundMessage) -> SendOperation {
        Box::pin(async move {
            Err(ChannelError::NotConnected(format!(
                "{RELAY_UNAVAILABLE}; nothing was sent"
            )))
        })
    }
}

pub struct VoiceCallChannelFactory;

#[async_trait]
impl ChannelFactory for VoiceCallChannelFactory {
    fn channel_type(&self) -> &str {
        "voice_call"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        self.create_with_id("voice_call", config).await
    }

    async fn create_with_id(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<Box<dyn Channel>> {
        let config: VoiceCallConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid voice_call config: {e}")))?;
        config.validate().map_err(ChannelError::ConfigError)?;
        Ok(Box::new(VoiceCallChannel::new(id, config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> serde_json::Value {
        serde_json::json!({
            "region": "US",
            "service_address": "+15550000000",
            "binding_ref": "core-binding-1",
        })
    }

    #[tokio::test]
    async fn registered_factory_preserves_instance_and_reports_relay_unavailable() {
        crate::gateway::interfaces::register_channel_plugins();
        let mut channel = crate::gateway::handlers::channel::create_channel_from_config(
            "phone-work",
            "voice_call",
            config(),
        )
        .await
        .expect("registered voice_call config must create a channel");
        assert_eq!(channel.id().as_str(), "phone-work");
        assert_eq!(channel.channel_type(), "voice_call");
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
        assert_eq!(
            channel.outbound_delivery_policy(),
            OutboundDeliveryPolicy::ExplicitOnly
        );
        let err = channel.start().await.unwrap_err();
        assert!(
            matches!(err, ChannelError::NotConnected(ref reason) if reason.contains("unavailable"))
        );
        assert_eq!(channel.status(), ChannelStatus::Error);
        assert_eq!(channel.info().status, ChannelStatus::Error);
        assert!(channel
            .health()
            .await
            .status_reason
            .unwrap()
            .contains("unavailable"));
        assert!(matches!(
            channel.send(OutboundMessage::text("call", "hello")).await,
            Err(ChannelError::NotConnected(_))
        ));
        channel.stop().await.unwrap();
        channel.stop().await.unwrap();
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
        assert_eq!(channel.info().status, ChannelStatus::Disconnected);
        assert!(channel.start().await.is_err());
        assert_eq!(channel.status(), ChannelStatus::Error);
    }

    #[tokio::test]
    async fn factory_rejects_missing_and_invalid_routing_fields() {
        let factory = VoiceCallChannelFactory;
        for field in ["region", "service_address", "binding_ref"] {
            let mut missing = config();
            missing.as_object_mut().unwrap().remove(field);
            assert!(matches!(
                factory.create(missing).await,
                Err(ChannelError::ConfigError(_))
            ));
        }
        for (field, value) in [
            ("region", "us"),
            ("service_address", "not-a-number"),
            ("binding_ref", " "),
        ] {
            let mut invalid = config();
            invalid[field] = value.into();
            assert!(matches!(
                factory.create(invalid).await,
                Err(ChannelError::ConfigError(_))
            ));
        }
    }

    #[test]
    fn config_roundtrip_preserves_switches_and_defaults() {
        let defaults: VoiceCallConfig = serde_json::from_value(config()).unwrap();
        assert!(defaults.inbound_enabled);
        assert!(defaults.outbound_enabled);
        let mut disabled = defaults;
        disabled.inbound_enabled = false;
        disabled.outbound_enabled = false;
        let restored: VoiceCallConfig =
            serde_json::from_value(serde_json::to_value(&disabled).unwrap()).unwrap();
        assert_eq!(restored, disabled);
        restored.validate().unwrap();
    }
}
