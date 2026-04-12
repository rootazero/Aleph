use crate::gateway::interfaces::whatsapp::native_baileys::client::BridgeEvent;

pub fn native_event_to_bridge_event(_event: &str) -> BridgeEvent {
    BridgeEvent::Unknown
}
