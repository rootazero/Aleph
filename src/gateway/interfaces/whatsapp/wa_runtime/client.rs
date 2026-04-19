use crate::gateway::channel::{ChannelError, ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use crate::gateway::interfaces::whatsapp::wa_runtime::http_client::ReqwestHttpClient;
use crate::gateway::interfaces::whatsapp::wa_runtime::state::{
    AtomicConnectionState, ConnectionState,
};
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, info, warn};

pub struct WaRuntime {
    state: Arc<AtomicConnectionState>,
    auth: WaAuthManager,
    event_tx: mpsc::Sender<whatsapp_rust::types::events::Event>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    bot_handle: Arc<Mutex<Option<whatsapp_rust::bot::BotHandle>>>,
    client: Arc<Mutex<Option<Arc<whatsapp_rust::Client>>>>,
    message_jids: Arc<Mutex<HashMap<String, whatsapp_rust::Jid>>>,
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
            client: Arc::new(Mutex::new(None)),
            message_jids: Arc::new(Mutex::new(HashMap::new())),
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
        let client_arc = Arc::clone(&self.client);
        let message_jids_arc = Arc::clone(&self.message_jids);

        tokio::spawn(async move {
            let event_tx = event_tx_for_bot.clone();
            let state = Arc::clone(&state_for_bot);
            let message_jids = Arc::clone(&message_jids_arc);

            let mut bot = match whatsapp_rust::bot::Bot::builder()
                .with_backend(backend)
                .with_transport_factory(transport)
                .with_http_client(http_client)
                .with_runtime(whatsapp_rust::TokioRuntime)
                .skip_history_sync()
                .on_event(move |event, _client| {
                    let event_tx = event_tx.clone();
                    let state = Arc::clone(&state);
                    let message_jids = Arc::clone(&message_jids);
                    async move {
                        handle_bot_event(event, state, event_tx, message_jids).await;
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

            {
                let mut guard = client_arc.lock().await;
                *guard = Some(bot.client());
            }

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
            {
                let mut guard = client_arc.lock().await;
                *guard = None;
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
            .map_err(|e| {
                ChannelError::Internal(format!("Failed to create SQLite backend: {}", e))
            })?;
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
        {
            let mut guard = self.client.lock().await;
            *guard = None;
        }
        {
            let mut guard = self.message_jids.lock().await;
            guard.clear();
        }
        self.state.set(ConnectionState::Disconnected);
    }

    async fn get_client(&self) -> ChannelResult<Arc<whatsapp_rust::Client>> {
        let guard = self.client.lock().await;
        guard
            .clone()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp client not ready".into()))
    }

    pub async fn send_message(&self, msg: OutboundMessage) -> ChannelResult<MessageId> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = msg
            .conversation_id
            .as_str()
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid JID: {}", e)))?;

        let wa_msg = if let Some(reply_to) = msg.reply_to {
            let context_info = whatsapp_rust::waproto::whatsapp::ContextInfo {
                stanza_id: Some(reply_to.as_str().to_string()),
                ..Default::default()
            };
            whatsapp_rust::waproto::whatsapp::Message {
                extended_text_message: Some(Box::new(
                    whatsapp_rust::waproto::whatsapp::message::ExtendedTextMessage {
                        text: Some(msg.text),
                        context_info: Some(Box::new(context_info)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
        } else {
            whatsapp_rust::waproto::whatsapp::Message {
                conversation: Some(msg.text),
                ..Default::default()
            }
        };

        let msg_id = client
            .send_message(jid, wa_msg)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send message: {}", e)))?;

        Ok(MessageId::new(msg_id))
    }

    pub async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = jid
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid JID: {}", e)))?;

        let reaction = whatsapp_rust::waproto::whatsapp::message::ReactionMessage {
            key: Some(whatsapp_rust::waproto::whatsapp::MessageKey {
                remote_jid: Some(jid.to_string()),
                id: Some(msg_id.to_string()),
                from_me: Some(false),
                participant: None,
            }),
            text: Some(emoji.to_string()),
            ..Default::default()
        };

        let wa_msg = whatsapp_rust::waproto::whatsapp::Message {
            reaction_message: Some(reaction),
            ..Default::default()
        };

        client
            .send_message(jid, wa_msg)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send reaction: {}", e)))?;

        Ok(())
    }

    pub async fn mark_read(&self, msg_id: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let chat = {
            let guard = self.message_jids.lock().await;
            guard.get(msg_id).cloned().ok_or_else(|| {
                ChannelError::Internal(format!("Unknown message ID for read receipt: {}", msg_id))
            })?
        };

        client
            .mark_as_read(&chat, None, vec![msg_id.to_string()])
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to send read receipt: {}", e)))?;

        Ok(())
    }

    pub async fn send_typing(&self, jid: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = jid
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid JID: {}", e)))?;

        client.chatstate().send_composing(&jid).await.map_err(|e| {
            ChannelError::Internal(format!("Failed to send typing indicator: {}", e))
        })?;

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
    message_jids: Arc<Mutex<HashMap<String, whatsapp_rust::Jid>>>,
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
        whatsapp_rust::types::events::Event::Message(_, info) => {
            let mut guard = message_jids.lock().await;
            guard.insert(info.id.to_string(), info.source.chat.clone());
        }
        _ => {}
    }

    if event_tx.send(event).await.is_err() {
        error!("Event receiver dropped");
    }
}
