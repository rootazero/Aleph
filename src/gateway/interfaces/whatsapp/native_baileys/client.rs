use crate::gateway::interfaces::whatsapp::native_baileys::errors::NativeBaileysError;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct NativeBaileysClient {
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl NativeBaileysClient {
    pub async fn new(
        _config: crate::gateway::interfaces::whatsapp::WhatsAppConfig,
        _event_tx: mpsc::Sender<BridgeEvent>,
    ) -> Result<Self, NativeBaileysError> {
        Ok(Self {
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn send_message(
        &self,
        _msg: crate::gateway::channel::OutboundMessage,
    ) -> Result<crate::gateway::channel::MessageId, NativeBaileysError> {
        Err(NativeBaileysError::NotConnected)
    }

    pub async fn send_reaction(
        &self,
        _msg_id: &crate::gateway::channel::MessageId,
        _emoji: &str,
    ) -> Result<(), NativeBaileysError> {
        Err(NativeBaileysError::NotConnected)
    }

    pub async fn mark_read(
        &self,
        _msg_id: &crate::gateway::channel::MessageId,
    ) -> Result<(), NativeBaileysError> {
        Err(NativeBaileysError::NotConnected)
    }
}

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    Connected,
    Disconnected(Option<String>),
    Messages(Vec<crate::gateway::channel::InboundMessage>),
    MessageUpdate,
    PresenceUpdate,
    Unknown,
}
