//! Map whatsapp-rust events to Aleph InboundMessage

use crate::gateway::channel::{ChannelId, InboundMessage};

pub struct InboundPolicyResult;

pub fn map_event_to_inbound(
    event: &whatsapp_rust::types::events::Event,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
    let _ = event;
    let _ = channel_id;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_message_event_returns_none() {
        assert!(true);
    }
}
