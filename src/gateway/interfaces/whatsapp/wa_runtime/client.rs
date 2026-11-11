use crate::gateway::channel::{ChannelError, ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::pairing::PairingState;
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use crate::gateway::interfaces::whatsapp::wa_runtime::http_client::ReqwestHttpClient;
use crate::gateway::interfaces::whatsapp::wa_runtime::state::{
    AtomicConnectionState, ConnectionState,
};
use crate::gateway::interfaces::whatsapp::wa_runtime::traits::{
    WaEvent, WaRuntime, WaRuntimeError,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, info, warn};

/// Production WhatsApp runtime.
///
/// Cheap to clone: every internal field is either `Arc` (so the clone
/// shares state) or a `Sender` (also `Clone`), except for `shutdown_tx`
/// which is now `Arc<Mutex<Option<...>>>` so the trait can drive it via
/// `&self` (Task 7). Cloning a `RealWaRuntime` no longer takes the
/// shutdown signal — the original instance owns it — and clones are
/// read-only observers. Sharing a clone with `ReactionHandler` is
/// therefore free at runtime; both holders route through the same
/// internal `Arc<Mutex<Option<Client>>>`.
pub struct RealWaRuntime {
    state: Arc<AtomicConnectionState>,
    auth: WaAuthManager,
    event_tx: mpsc::Sender<WaEvent>,
    /// `Arc<Mutex<Option<...>>>` (instead of `Option<...>`) so
    /// `start()` / `shutdown()` can be called via `&self` for trait
    /// object safety. Only the original `RealWaRuntime` mutates this;
    /// clones see the same value through the shared `Arc`.
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    supervisor: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    client: Arc<Mutex<Option<Arc<whatsapp_rust::Client>>>>,
    message_jids: Arc<Mutex<HashMap<String, whatsapp_rust::Jid>>>,
}

impl Clone for RealWaRuntime {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            auth: self.auth.clone(),
            event_tx: self.event_tx.clone(),
            // Shared via `Arc` so the original instance's `start()` /
            // `shutdown()` writes are visible to clones. Clones never
            // *write* through it; they only observe via `pairing_phase`
            // etc. This is the change that lets the trait method take
            // `&self`.
            shutdown_tx: Arc::clone(&self.shutdown_tx),
            supervisor: Arc::clone(&self.supervisor),
            client: Arc::clone(&self.client),
            message_jids: Arc::clone(&self.message_jids),
        }
    }
}

impl RealWaRuntime {
    pub async fn new(auth: WaAuthManager, event_tx: mpsc::Sender<WaEvent>) -> ChannelResult<Self> {
        Ok(Self {
            state: Arc::new(AtomicConnectionState::new(ConnectionState::Disconnected)),
            auth,
            event_tx,
            shutdown_tx: Arc::new(Mutex::new(None)),
            supervisor: Arc::new(Mutex::new(None)),
            client: Arc::new(Mutex::new(None)),
            message_jids: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[must_use]
    pub fn connection_state(&self) -> ConnectionState {
        self.state.get()
    }

    #[must_use]
    pub fn state_handle(&self) -> Arc<AtomicConnectionState> {
        Arc::clone(&self.state)
    }

    async fn create_backend(
        &self,
        db_path: &str,
    ) -> ChannelResult<Arc<dyn whatsapp_rust::store::traits::Backend>> {
        let backend = whatsapp_rust::store::SqliteStore::new(db_path)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create SQLite backend: {e}")))?;
        Ok(Arc::new(backend) as Arc<dyn whatsapp_rust::store::traits::Backend>)
    }

    async fn get_client(&self) -> ChannelResult<Arc<whatsapp_rust::Client>> {
        let guard = self.client.lock().await;
        guard
            .clone()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp client not ready".into()))
    }

    /// Internal `start`-mapped method. The trait method `start` is
    /// `&self`; this helper is `&mut self`-free because all mutable
    /// state lives behind `Arc<Mutex<...>>` already (Task 7 refactor).
    async fn start_inner(&self) -> ChannelResult<()> {
        info!("Starting WhatsApp runtime...");

        // This lock owns Starting as well as Running, and serializes replacement with shutdown.
        let mut supervisor = self.supervisor.lock().await;
        if supervisor.as_ref().is_some_and(|task| !task.is_finished()) {
            return Err(ChannelError::Internal(
                "WhatsApp runtime already started".into(),
            ));
        }
        if let Some(task) = supervisor.take() {
            let _ = task.await;
        }
        let db_path = self.auth.db_path();
        let backend = self.create_backend(&db_path).await?;
        let transport = whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory::new();
        let http_client = ReqwestHttpClient::new();

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let event_tx_for_bot = self.event_tx.clone();
        let state_for_bot = Arc::clone(&self.state);
        let client_arc = Arc::clone(&self.client);
        let message_jids_arc = Arc::clone(&self.message_jids);
        let shutdown_tx_for_task = Arc::clone(&self.shutdown_tx);

        *supervisor = Some(tokio::spawn(async move {
            let event_tx = event_tx_for_bot.clone();
            let state = Arc::clone(&state_for_bot);
            let message_jids = Arc::clone(&message_jids_arc);

            let build = whatsapp_rust::bot::Bot::builder()
                .with_backend_arc(backend)
                .with_transport_factory(transport)
                .with_http_client(http_client)
                .with_runtime(whatsapp_rust::TokioRuntime)
                .skip_history_sync()
                .on_event(move |event, _client| {
                    let event_tx = event_tx.clone();
                    let state = Arc::clone(&state);
                    let message_jids = Arc::clone(&message_jids);
                    async move {
                        handle_bot_event((*event).clone(), state, event_tx, message_jids).await;
                    }
                })
                .build();
            let result = tokio::select! {
                biased;
                _ = &mut shutdown_rx => {
                    state_for_bot.set(ConnectionState::Disconnected);
                    return;
                }
                result = build => result,
            };
            let bot = match result {
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

            let handle = bot.spawn();
            let _ = shutdown_rx.await;
            handle.shutdown().await;
            {
                let mut guard = client_arc.lock().await;
                *guard = None;
            }
            // Clear the channel's stored shutdown sender so a future
            // `start()` can install a fresh one.
            let mut guard = shutdown_tx_for_task.lock().await;
            *guard = None;
            state_for_bot.set(ConnectionState::Disconnected);
        }));

        Ok(())
    }

    /// Internal `shutdown`-mapped method. See `start_inner` for why
    /// this is now `&self`.
    async fn shutdown_inner(&self) {
        info!("Shutting down WhatsApp runtime...");
        let mut supervisor = self.supervisor.lock().await;
        let tx = self.shutdown_tx.lock().await.take();
        if let Some(tx) = tx {
            let _ = tx.send(());
        }
        if let Some(task) = supervisor.as_mut() {
            let _ = task.await;
        }
        *supervisor = None;
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

    pub async fn send_message(&self, msg: OutboundMessage) -> ChannelResult<MessageId> {
        if !msg.attachments.is_empty() {
            return Err(ChannelError::UnsupportedFeature(
                "WhatsApp attachment sending is not implemented; no message was sent".into(),
            ));
        }
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = msg
            .conversation_id
            .as_str()
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid JID: {e}")))?;

        let wa_msg = if let Some(reply_to) = msg.reply_to {
            let context_info = whatsapp_rust::waproto::whatsapp::ContextInfo {
                stanza_id: Some(reply_to.as_str().to_string()),
                ..Default::default()
            };
            whatsapp_rust::waproto::whatsapp::Message {
                extended_text_message:
                    whatsapp_rust::waproto::whatsapp::message::ExtendedTextMessage {
                        text: Some(msg.text),
                        context_info: context_info.into(),
                        ..Default::default()
                    }
                    .into(),
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
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send message: {e}")))?;

        Ok(MessageId::new(msg_id.message_id))
    }

    pub async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = jid
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid JID: {e}")))?;

        let reaction = whatsapp_rust::waproto::whatsapp::message::ReactionMessage {
            key: whatsapp_rust::waproto::whatsapp::MessageKey {
                remote_jid: Some(jid.to_string()),
                id: Some(msg_id.to_string()),
                from_me: Some(false),
                participant: None,
            }
            .into(),
            text: Some(emoji.to_string()),
            ..Default::default()
        };

        let wa_msg = whatsapp_rust::waproto::whatsapp::Message {
            reaction_message: reaction.into(),
            ..Default::default()
        };

        client
            .send_message(jid, wa_msg)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send reaction: {e}")))?;

        Ok(())
    }

    pub async fn mark_read(&self, msg_id: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let chat = {
            let guard = self.message_jids.lock().await;
            guard.get(msg_id).cloned().ok_or_else(|| {
                ChannelError::Internal(format!("Unknown message ID for read receipt: {msg_id}"))
            })?
        };

        client
            .mark_as_read(&chat, None, &[msg_id])
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to send read receipt: {e}")))?;

        Ok(())
    }

    pub async fn send_typing(&self, jid: &str) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = jid
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid JID: {e}")))?;

        client
            .chatstate()
            .send_composing(&jid)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to send typing indicator: {e}")))?;

        Ok(())
    }

    /// Set the profile picture of a group conversation.
    ///
    /// Wire shape (per wacore 0.5 `SetProfilePictureSpec`):
    /// ```xml
    /// <iq type="set" xmlns="w:profile:picture" to="<group_jid>">
    ///   <picture type="image">{jpeg bytes}</picture>
    /// </iq>
    /// ```
    ///
    /// WhatsApp's protocol doesn't have a "remove group icon" path
    /// distinguishable from setting an empty one; the channel layer
    /// treats `image_data` as required and never calls this with
    /// empty bytes.
    pub async fn set_group_picture(
        &self,
        group_jid: &str,
        image_data: Vec<u8>,
    ) -> ChannelResult<()> {
        self.ensure_connected()?;
        let client = self.get_client().await?;
        let jid: whatsapp_rust::Jid = group_jid
            .parse()
            .map_err(|e| ChannelError::Internal(format!("Invalid group JID: {e}")))?;

        let spec = wacore::iq::contacts::SetProfilePictureSpec::set_group(&jid, image_data);
        client
            .execute(spec)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to set group icon: {e}")))?;
        Ok(())
    }

    fn ensure_connected(&self) -> ChannelResult<()> {
        if self.state.get() != ConnectionState::Connected {
            return Err(ChannelError::NotConnected("WhatsApp not connected".into()));
        }
        Ok(())
    }
}

#[async_trait]
impl WaRuntime for RealWaRuntime {
    async fn start(&self) -> Result<(), WaRuntimeError> {
        self.start_inner()
            .await
            .map_err(|e| WaRuntimeError::Internal(e.to_string()))
    }

    async fn shutdown(&self) {
        self.shutdown_inner().await;
    }

    async fn pairing_phase(&self) -> PairingState {
        // `PairingState` is the single source of truth for
        // `WhatsAppChannel::status()` (spec §3.1 D1). The legacy
        // atomic `ConnectionState` is kept for backward compat with
        // `ensure_connected()` and `state_handle()`; we derive the
        // pairing phase from it rather than maintaining two parallel
        // state machines. Reconnection / failure paths funnel through
        // `Event::Disconnected` and `Event::PairError` which the event
        // loop's `PairingStateDriver` maps to the full 9-variant FSM.
        match self.state.get() {
            ConnectionState::Disconnected => PairingState::Disconnected {
                reason: "runtime disconnected".to_string(),
            },
            ConnectionState::Pairing => PairingState::WaitingQr {
                qr_data: String::new(),
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
            },
            ConnectionState::Connecting => PairingState::Initializing,
            ConnectionState::Connected => PairingState::Connected {
                device_name: String::new(),
                phone_number: String::new(),
            },
            ConnectionState::Error => PairingState::Failed {
                error: "runtime error".to_string(),
            },
        }
    }

    async fn send_message(&self, msg: OutboundMessage) -> Result<MessageId, WaRuntimeError> {
        Self::send_message(self, msg).await.map_err(|e| match e {
            ChannelError::NotConnected(m) => WaRuntimeError::NotConnected(m),
            ChannelError::SendFailed(m) => WaRuntimeError::SendFailed(m),
            other => WaRuntimeError::Internal(other.to_string()),
        })
    }

    async fn send_typing(&self, conversation_id: &str) -> Result<(), WaRuntimeError> {
        Self::send_typing(self, conversation_id)
            .await
            .map_err(|e| match e {
                ChannelError::NotConnected(m) => WaRuntimeError::NotConnected(m),
                other => WaRuntimeError::Internal(other.to_string()),
            })
    }

    async fn mark_read(&self, message_id: &str) -> Result<(), WaRuntimeError> {
        Self::mark_read(self, message_id)
            .await
            .map_err(|e| match e {
                ChannelError::NotConnected(m) => WaRuntimeError::NotConnected(m),
                other => WaRuntimeError::Internal(other.to_string()),
            })
    }

    async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), WaRuntimeError> {
        Self::send_reaction(self, conversation_id, message_id, emoji)
            .await
            .map_err(|e| match e {
                ChannelError::NotConnected(m) => WaRuntimeError::NotConnected(m),
                other => WaRuntimeError::Internal(other.to_string()),
            })
    }

    async fn set_group_picture(
        &self,
        group_jid: &str,
        image_data: Vec<u8>,
    ) -> Result<(), WaRuntimeError> {
        Self::set_group_picture(self, group_jid, image_data)
            .await
            .map_err(|e| match e {
                ChannelError::NotConnected(m) => WaRuntimeError::NotConnected(m),
                other => WaRuntimeError::Internal(other.to_string()),
            })
    }

    fn take_event_receiver(&self) -> Option<mpsc::Receiver<WaEvent>> {
        // The production runtime does not own its event channel —
        // `WhatsAppChannel::start()` passes a `Sender` in via
        // `RealWaRuntime::new` and retains the `Receiver` for its own
        // event loop. There is nothing to "take" here; the trait
        // method exists so the fake can hand its receiver to test
        // code.
        None
    }
}

async fn handle_bot_event(
    event: WaEvent,
    state: Arc<AtomicConnectionState>,
    event_tx: mpsc::Sender<WaEvent>,
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
        whatsapp_rust::types::events::Event::PairingQrCode(_) => {
            info!("QR code generated for pairing");
            state.set(ConnectionState::Pairing);
        }
        whatsapp_rust::types::events::Event::PairSuccess(_) => {
            info!("Pairing successful");
        }
        whatsapp_rust::types::events::Event::Messages(batch) => {
            let mut guard = message_jids.lock().await;
            // Evict only the minimum number of old entries needed to
            // fit the entire batch, rather than clearing the whole map.
            // Clearing could erase receipt targets for messages in
            // this same batch that have not yet been forwarded.
            if guard.len() + batch.len() > 10_000 {
                let excess = guard.len() + batch.len() - 10_000;
                let to_remove: Vec<String> = guard.keys().take(excess).cloned().collect();
                for k in to_remove {
                    guard.remove(&k);
                }
            }
            for message in batch {
                let info = &message.info;
                guard.insert(info.id.to_string(), info.source.chat.clone());
            }
        }
        _ => {}
    }

    if event_tx.send(event).await.is_err() {
        error!("Event receiver dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        channel::Channel,
        interfaces::whatsapp::{WhatsAppChannel, WhatsAppConfig},
    };
    use crate::secrets::vault::SecretVault;

    #[tokio::test]
    async fn stop_joins_starting_runtime_and_suspended_channel_loop() {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let dir = tempfile::tempdir().unwrap();
            let auth = WaAuthManager::with_vault(
                SecretVault::open(dir.path().join("auth.vault")).unwrap(),
                "test",
            );
            let (tx, _rx) = mpsc::channel(1);
            let runtime = RealWaRuntime::new(auth, tx).await.unwrap();
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let (stopping_tx, stopping_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            *runtime.shutdown_tx.lock().await = Some(shutdown_tx);
            let state = runtime.state_handle();
            *runtime.supervisor.lock().await = Some(tokio::spawn(async move {
                let _ = shutdown_rx.await;
                let _ = stopping_tx.send(());
                let _ = release_rx.await;
                state.set(ConnectionState::Disconnected);
            }));
            assert!(
                runtime.start_inner().await.is_err(),
                "Starting already owns the runtime"
            );
            let stopping = runtime.clone();
            let stop = tokio::spawn(async move { stopping.shutdown_inner().await });
            stopping_rx.await.unwrap();
            assert!(!stop.is_finished(), "shutdown must join the old builder");
            release_tx.send(()).unwrap();
            stop.await.unwrap();
            assert!(runtime.supervisor.lock().await.is_none());

            let (mut channel, _fake) =
                WhatsAppChannel::for_test_with_fake("wa-test", WhatsAppConfig::default());
            let pairing = Arc::clone(&channel.pairing_state);
            let (entered_tx, entered_rx) = oneshot::channel();
            let task = tokio::spawn(async move {
                let _ = entered_tx.send(());
                std::future::pending::<()>().await;
                *pairing.write().await = PairingState::Idle;
            });
            let finished = task.abort_handle();
            channel.event_task = Some(task);
            entered_rx.await.unwrap();
            assert!(channel.start().await.is_err());
            channel.stop().await.unwrap();
            assert!(
                finished.is_finished(),
                "stop must join even a loop suspended in acknowledgement"
            );
            channel.start().await.unwrap();
            tokio::task::yield_now().await;
            assert!(matches!(
                channel.pairing_state().await,
                PairingState::Connected { .. }
            ));
            channel.stop().await.unwrap();
        })
        .await
        .expect("owned lifecycle tasks must finish");
    }
}
