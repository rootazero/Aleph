use crate::gateway::channel::{ChannelError, ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use crate::gateway::interfaces::whatsapp::wa_runtime::state::{AtomicConnectionState, ConnectionState};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct WaRuntime {
    state: Arc<AtomicConnectionState>,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    auth: WaAuthManager,
}

impl WaRuntime {
    pub async fn new(
        auth: WaAuthManager,
        event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    ) -> ChannelResult<Self> {
        Ok(Self {
            state: Arc::new(AtomicConnectionState::new(ConnectionState::Disconnected)),
            event_tx,
            shutdown_tx: None,
            auth,
        })
    }

    pub fn connection_state(&self) -> ConnectionState {
        self.state.get()
    }

    pub async fn start(&mut self) -> ChannelResult<()> {
        self.state.set(ConnectionState::Connecting);

        if self.auth.exists() {
            self.state.set(ConnectionState::Connected);
        }

        Ok(())
    }

    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        self.state.set(ConnectionState::Disconnected);
    }

    pub async fn send_message(&self, _msg: OutboundMessage) -> ChannelResult<MessageId> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected("WhatsApp not connected".into()));
        }
        Ok(MessageId::new("wa-msg-id"))
    }

    pub async fn send_reaction(&self, _jid: &str, _msg_id: &str, _emoji: &str) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected("WhatsApp not connected".into()));
        }
        Ok(())
    }

    pub async fn mark_read(&self, _jid: &str, _msg_id: &str) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected("WhatsApp not connected".into()));
        }
        Ok(())
    }

    pub async fn send_typing(&self, _jid: &str) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected("WhatsApp not connected".into()));
        }
        Ok(())
    }

    pub fn state_handle(&self) -> Arc<AtomicConnectionState> {
        Arc::clone(&self.state)
    }
}
