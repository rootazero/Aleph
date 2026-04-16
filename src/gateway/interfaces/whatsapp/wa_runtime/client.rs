use crate::gateway::interfaces::whatsapp::wa_runtime::http_client::ReqwestHttpClient;
use crate::gateway::interfaces::whatsapp::wa_runtime::state::{AtomicConnectionState, ConnectionState};
use crate::gateway::channel::{ChannelError, ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, info, warn};

pub struct WaRuntime {
    state: Arc<AtomicConnectionState>,
    auth: WaAuthManager,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    bot_handle: Arc<Mutex<Option<whatsapp_rust::bot::BotHandle>>>,
}

impl WaRuntime {
    pub async fn new(
        auth: WaAuthManager,
        event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    ) -> ChannelResult<Self> {
        Ok(Self {
            state: Arc::new(AtomicConnectionState::new(ConnectionState::Disconnected)),
            auth,
            event_tx,
            shutdown_tx: None,
            bot_handle: Arc::new(Mutex::new(None)),
        })
    }

    pub fn connection_state(&self) -> ConnectionState {
        self.state.get()
    }

    pub fn state_handle(&self) -> Arc<AtomicConnectionState> {
        Arc::clone(&self.state)
    }

    pub async fn start(&mut self) -> ChannelResult<()> {
        info!("Starting WhatsApp runtime...");

        let db_path = self.auth.db_path();
        let backend = self.create_backend(&db_path).await?;
        let transport = whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory::new();
        let http_client = ReqwestHttpClient::new();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let event_tx_for_bot = self.event_tx.clone();
        let state_for_bot = Arc::clone(&self.state);
        let bot_handle_arc = Arc::clone(&self.bot_handle);

        tokio::spawn(async move {
            let event_tx = event_tx_for_bot.clone();
            let state = Arc::clone(&state_for_bot);
            
            let mut bot = match whatsapp_rust::bot::Bot::builder()
                .with_backend(backend)
                .with_transport_factory(transport)
                .with_http_client(http_client)
                .with_runtime(whatsapp_rust::TokioRuntime)
                .skip_history_sync()
                .on_event(move |event, _client| {
                    let event_tx = event_tx.clone();
                    let state = Arc::clone(&state);
                    async move {
                        handle_bot_event(event, state, event_tx).await;
                    }
                })
                .build()
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    error!(error = %e, "Failed to build bot");
                    state_for_bot.set(ConnectionState::Error);
                    return;
                }
            };

            let handle = match bot.run().await {
                Ok(h) => h,
                Err(e) => {
                    error!(error = %e, "Failed to run bot");
                    state_for_bot.set(ConnectionState::Error);
                    return;
                }
            };

            {
                let mut guard = bot_handle_arc.lock().await;
                *guard = Some(handle);
            }

            let _ = shutdown_rx.await;
            {
                let mut guard = bot_handle_arc.lock().await;
                if let Some(h) = guard.take() {
                    h.abort();
                }
            }
            state_for_bot.set(ConnectionState::Disconnected);
        });

        Ok(())
    }

    async fn create_backend(
        &self,
        db_path: &str,
    ) -> ChannelResult<Arc<dyn whatsapp_rust::store::traits::Backend>> {
        let backend = whatsapp_rust::store::SqliteStore::new(db_path)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create SQLite backend: {}", e)))?;
        Ok(Arc::new(backend) as Arc<dyn whatsapp_rust::store::traits::Backend>)
    }

    pub async fn shutdown(&mut self) {
        info!("Shutting down WhatsApp runtime...");
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        {
            let mut guard = self.bot_handle.lock().await;
            if let Some(h) = guard.take() {
                h.abort();
            }
        }
        self.state.set(ConnectionState::Disconnected);
    }

    pub async fn send_message(&self, _msg: OutboundMessage) -> ChannelResult<MessageId> {
        self.ensure_connected()?;
        warn!("send_message not yet implemented with real bot API");
        Ok(MessageId::new("wa-msg-id"))
    }

    pub async fn send_reaction(&self, _jid: &str, _msg_id: &str, _emoji: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        warn!("send_reaction not yet implemented");
        Ok(())
    }

    pub async fn mark_read(&self, _jid: &str, _msg_id: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        warn!("mark_read not yet implemented");
        Ok(())
    }

    pub async fn send_typing(&self, _jid: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        warn!("send_typing not yet implemented");
        Ok(())
    }

    fn ensure_connected(&self) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected("WhatsApp not connected".into()));
        }
        Ok(())
    }
}

async fn handle_bot_event(
    event: whatsapp_rust::types::events::Event,
    state: Arc<AtomicConnectionState>,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
) {
    match &event {
        whatsapp_rust::types::events::Event::Connected(_) => {
            info!("WhatsApp connection opened");
            state.set(ConnectionState::Connected);
        }
        whatsapp_rust::types::events::Event::Disconnected(_) => {
            warn!("WhatsApp connection closed");
            state.set(ConnectionState::Disconnected);
        }
        whatsapp_rust::types::events::Event::PairingQrCode { .. } => {
            info!("QR code generated for pairing");
            state.set(ConnectionState::Pairing);
        }
        whatsapp_rust::types::events::Event::PairSuccess(_) => {
            info!("Pairing successful");
        }
        _ => {}
    }

    if event_tx.send(event).await.is_err() {
        error!("Event receiver dropped");
    }
}
