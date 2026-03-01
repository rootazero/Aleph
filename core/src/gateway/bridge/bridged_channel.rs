//! BridgedChannel -- a channel proxy that delegates all operations to an
//! external bridge process via a [`Transport`].
//!
//! # Design rationale
//!
//! The existing [`Channel`] trait requires `fn info(&self) -> &ChannelInfo`,
//! a synchronous borrow that conflicts with interior mutability for dynamic
//! status updates from the bridge.  Rather than forcing an unsafe workaround,
//! `BridgedChannel` provides **equivalent async-friendly methods** and leaves
//! the trait adaptation to the higher-level `LinkManager` (Task 6).
//!
//! [`Channel`]: crate::gateway::channel::Channel
//! [`Transport`]: crate::gateway::transport::Transport

use crate::sync_primitives::Arc;

use base64::Engine as _;
use chrono::{DateTime, TimeZone, Utc};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::gateway::channel::{
    Attachment, ChannelCapabilities, ChannelError, ChannelId, ChannelResult, ChannelStatus,
    ConversationId, InboundMessage, MessageId, OutboundMessage, PairingData, SendResult, UserId,
};
use crate::gateway::transport::{AttachmentPayload, BridgeEvent, PairingEvent, Transport};

// ---------------------------------------------------------------------------
// Status wrapper (shared between BridgedChannel and event loop)
// ---------------------------------------------------------------------------

/// Thread-safe, shared channel status.
///
/// Uses a `std::sync::RwLock` (not tokio's) so that synchronous readers
/// (e.g. `fn status()`) can access the value without an async context.
#[derive(Debug)]
struct SharedStatus(std::sync::RwLock<ChannelStatus>);

impl SharedStatus {
    fn new(status: ChannelStatus) -> Arc<Self> {
        Arc::new(Self(std::sync::RwLock::new(status)))
    }

    fn get(&self) -> ChannelStatus {
        *self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, status: ChannelStatus) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = status;
    }
}

// ---------------------------------------------------------------------------
// BridgedChannel
// ---------------------------------------------------------------------------

/// A channel proxy that delegates all operations to an external bridge
/// process via a [`Transport`] (Unix socket or stdio).
///
/// This struct is **not** a direct implementor of the [`Channel`] trait.
/// See the module-level docs for rationale.
///
/// # Lifecycle
///
/// 1. Create with [`BridgedChannel::new`].
/// 2. Attach a transport with [`set_transport`](BridgedChannel::set_transport).
/// 3. Optionally tweak capabilities with [`set_capabilities`](BridgedChannel::set_capabilities).
/// 4. Call [`start`](BridgedChannel::start) to begin event forwarding.
/// 5. Take the inbound receiver with [`take_inbound_receiver`](BridgedChannel::take_inbound_receiver)
///    and hand it to the message router.
/// 6. Send outbound messages with [`send`](BridgedChannel::send).
/// 7. Call [`stop`](BridgedChannel::stop) when shutting down.
pub struct BridgedChannel {
    id: ChannelId,
    name: String,
    bridge_id: String,
    capabilities: ChannelCapabilities,
    status: Arc<SharedStatus>,
    transport: Option<Arc<dyn Transport>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl BridgedChannel {
    /// Create a new `BridgedChannel`.
    ///
    /// The channel starts in [`ChannelStatus::Disconnected`] with default
    /// (empty) capabilities.  A transport must be attached via
    /// [`set_transport`](BridgedChannel::set_transport) before calling
    /// [`start`](BridgedChannel::start).
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        bridge_id: impl Into<String>,
    ) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        Self {
            id: ChannelId::new(id),
            name: name.into(),
            bridge_id: bridge_id.into(),
            capabilities: ChannelCapabilities::default(),
            status: SharedStatus::new(ChannelStatus::Disconnected),
            transport: None,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            shutdown_tx: None,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Channel identifier.
    pub fn id(&self) -> &ChannelId {
        &self.id
    }

    /// Human-readable channel name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Identifier of the bridge plugin backing this channel.
    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    /// Channel type string (always `"bridged"`).
    pub fn channel_type(&self) -> &str {
        "bridged"
    }

    /// Declared capabilities.
    pub fn capabilities(&self) -> &ChannelCapabilities {
        &self.capabilities
    }

    /// Current connection status (synchronous read).
    pub fn status(&self) -> ChannelStatus {
        self.status.get()
    }

    // -----------------------------------------------------------------------
    // Configuration (call before start)
    // -----------------------------------------------------------------------

    /// Attach the IPC transport to use for bridge communication.
    pub fn set_transport(&mut self, transport: Arc<dyn Transport>) {
        self.transport = Some(transport);
    }

    /// Override the default capabilities.
    pub fn set_capabilities(&mut self, caps: ChannelCapabilities) {
        self.capabilities = caps;
    }

    /// Take the inbound message receiver.
    ///
    /// Intended to be called **once** by the message router.  Subsequent
    /// calls return `None`.
    pub fn take_inbound_receiver(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.take()
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Start the channel by sending `aleph.link.start` to the bridge and
    /// spawning the event-forwarding loop.
    pub async fn start(&mut self) -> ChannelResult<()> {
        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| ChannelError::Internal("Transport not set".into()))?
            .clone();

        self.status.set(ChannelStatus::Connecting);
        info!(channel = %self.id, "Starting bridged channel");

        // Ask the bridge to begin operation.
        transport
            .request("aleph.link.start", serde_json::json!({}))
            .await
            .map_err(|e| ChannelError::Internal(format!("start request failed: {e}")))?;

        // Spawn the background event loop.
        let shutdown_tx = spawn_event_loop(
            transport,
            self.id.clone(),
            self.inbound_tx.clone(),
            Arc::clone(&self.status),
        );
        self.shutdown_tx = Some(shutdown_tx);

        Ok(())
    }

    /// Stop the channel, signalling the event loop and the bridge process.
    pub async fn stop(&mut self) -> ChannelResult<()> {
        info!(channel = %self.id, "Stopping bridged channel");

        // Signal the event loop to exit.
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Tell the bridge to shut down, then close the transport.
        // Use a timeout since the transport may already be dead.
        if let Some(transport) = &self.transport {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                transport.request("aleph.link.stop", serde_json::json!({})),
            )
            .await;
            let _ = transport.close().await;
        }

        self.status.set(ChannelStatus::Disconnected);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Messaging
    // -----------------------------------------------------------------------

    /// Send an outbound message through the bridge.
    pub async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| ChannelError::Internal("Transport not set".into()))?;

        let params = serde_json::json!({
            "conversation_id": message.conversation_id.as_str(),
            "text": message.text,
            "reply_to": message.reply_to.as_ref().map(|m| m.as_str()),
        });

        let resp = transport
            .request("aleph.link.send", params)
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;

        let message_id = resp
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Prefer the bridge-provided timestamp if available, fall back to local clock.
        let timestamp = resp
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
            .unwrap_or_else(Utc::now);

        Ok(SendResult {
            message_id: MessageId::new(message_id),
            timestamp,
        })
    }

    /// Request pairing data from the bridge (e.g. QR code, pairing code).
    pub async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        let transport = match &self.transport {
            Some(t) => t,
            None => return Ok(PairingData::None),
        };

        let resp = transport
            .request("aleph.link.get_pairing", serde_json::json!({}))
            .await
            .map_err(|e| ChannelError::Internal(e.to_string()))?;

        if let Some(qr) = resp.get("qr_data").and_then(|v| v.as_str()) {
            Ok(PairingData::QrCode(qr.to_string()))
        } else if let Some(code) = resp.get("code").and_then(|v| v.as_str()) {
            Ok(PairingData::Code(code.to_string()))
        } else {
            Ok(PairingData::None)
        }
    }

    // -----------------------------------------------------------------------
    // Internal: status mutation (for testing)
    // -----------------------------------------------------------------------

    /// Set the channel status directly.  Used internally and in tests.
    #[allow(dead_code)]
    fn set_status(&self, status: ChannelStatus) {
        self.status.set(status);
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Spawn a background task that reads [`BridgeEvent`]s from the transport
/// and forwards them as [`InboundMessage`]s on the inbound channel.
fn spawn_event_loop(
    transport: Arc<dyn Transport>,
    channel_id: ChannelId,
    inbound_tx: mpsc::Sender<InboundMessage>,
    status: Arc<SharedStatus>,
) -> oneshot::Sender<()> {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        debug!(channel = %channel_id, "Event loop started");

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!(channel = %channel_id, "Event loop received shutdown signal");
                    break;
                }
                event = transport.next_event() => {
                    match event {
                        Some(ev) => handle_event(&ev, &channel_id, &inbound_tx, &status).await,
                        None => {
                            // Transport closed; mark disconnected and exit.
                            warn!(channel = %channel_id, "Transport closed, stopping event loop");
                            status.set(ChannelStatus::Disconnected);
                            break;
                        }
                    }
                }
            }
        }

        debug!(channel = %channel_id, "Event loop exited");
    });

    shutdown_tx
}

/// Process a single [`BridgeEvent`].
async fn handle_event(
    event: &BridgeEvent,
    channel_id: &ChannelId,
    inbound_tx: &mpsc::Sender<InboundMessage>,
    status: &SharedStatus,
) {
    match event {
        BridgeEvent::Ready => {
            info!(channel = %channel_id, "Bridge reported ready");
            status.set(ChannelStatus::Connected);
        }

        BridgeEvent::StatusChange { status: new_status } => {
            debug!(channel = %channel_id, status = %new_status, "Bridge status change");
            let cs = match new_status.as_str() {
                "connected" => ChannelStatus::Connected,
                "connecting" => ChannelStatus::Connecting,
                "disconnected" => ChannelStatus::Disconnected,
                "error" => ChannelStatus::Error,
                _ => {
                    warn!(channel = %channel_id, status = %new_status, "Unknown bridge status");
                    ChannelStatus::Error
                }
            };
            status.set(cs);
        }

        BridgeEvent::PairingUpdate(pairing) => {
            debug!(channel = %channel_id, ?pairing, "Pairing update");
            match pairing {
                PairingEvent::Connected { .. } => {
                    status.set(ChannelStatus::Connected);
                }
                PairingEvent::Failed { error } => {
                    error!(channel = %channel_id, %error, "Pairing failed");
                    status.set(ChannelStatus::Error);
                }
                _ => { /* other pairing events are informational */ }
            }
        }

        BridgeEvent::Message {
            from,
            sender_name,
            conversation_id,
            text,
            message_id,
            timestamp,
            is_group,
            attachments,
            reply_to,
        } => {
            let ts: DateTime<Utc> = Utc
                .timestamp_opt(*timestamp, 0)
                .single()
                .unwrap_or_else(Utc::now);

            let converted_attachments: Vec<Attachment> = attachments
                .iter()
                .map(convert_attachment)
                .collect();

            let inbound = InboundMessage {
                id: MessageId::new(message_id.clone()),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(conversation_id.clone()),
                sender_id: UserId::new(from.clone()),
                sender_name: sender_name.clone(),
                text: text.clone(),
                attachments: converted_attachments,
                timestamp: ts,
                reply_to: reply_to.as_ref().map(|r| MessageId::new(r.clone())),
                is_group: *is_group,
                raw: None,
            };

            if let Err(e) = inbound_tx.send(inbound).await {
                error!(channel = %channel_id, %e, "Failed to forward inbound message");
            }
        }

        BridgeEvent::Receipt {
            message_id,
            receipt_type,
        } => {
            debug!(
                channel = %channel_id,
                %message_id,
                %receipt_type,
                "Received delivery receipt (not yet forwarded)"
            );
            // Receipts will be forwarded once the event bus is integrated.
        }

        BridgeEvent::Error { message } => {
            error!(channel = %channel_id, %message, "Bridge reported error");
            status.set(ChannelStatus::Error);
        }
    }
}

/// Convert a bridge [`AttachmentPayload`] into a channel [`Attachment`].
fn convert_attachment(payload: &AttachmentPayload) -> Attachment {
    let decoded = match base64::engine::general_purpose::STANDARD.decode(&payload.data) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            warn!(
                mime_type = %payload.mime_type,
                error = %e,
                "Attachment base64 decode failed; dropping data"
            );
            None
        }
    };

    Attachment {
        id: uuid::Uuid::new_v4().to_string(),
        mime_type: payload.mime_type.clone(),
        filename: payload.filename.clone(),
        size: None,
        url: None,
        path: None,
        data: decoded,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridged_channel_creation() {
        let ch = BridgedChannel::new("whatsapp:alice", "Alice (WhatsApp)", "whatsapp");
        assert_eq!(ch.id().as_str(), "whatsapp:alice");
        assert_eq!(ch.name(), "Alice (WhatsApp)");
        assert_eq!(ch.bridge_id(), "whatsapp");
        assert_eq!(ch.channel_type(), "bridged");
        assert_eq!(ch.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_bridged_channel_set_capabilities() {
        let mut ch = BridgedChannel::new("test:1", "Test", "test");
        assert!(!ch.capabilities().attachments);
        assert!(!ch.capabilities().reactions);

        ch.set_capabilities(ChannelCapabilities {
            attachments: true,
            images: true,
            reactions: true,
            ..Default::default()
        });

        assert!(ch.capabilities().attachments);
        assert!(ch.capabilities().images);
        assert!(ch.capabilities().reactions);
        assert!(!ch.capabilities().audio);
    }

    #[test]
    fn test_take_inbound_receiver() {
        let mut ch = BridgedChannel::new("test:2", "Test", "test");

        // First take succeeds.
        let rx = ch.take_inbound_receiver();
        assert!(rx.is_some());

        // Second take returns None.
        let rx2 = ch.take_inbound_receiver();
        assert!(rx2.is_none());
    }

    #[test]
    fn test_status_change() {
        let ch = BridgedChannel::new("test:3", "Test", "test");
        assert_eq!(ch.status(), ChannelStatus::Disconnected);

        ch.set_status(ChannelStatus::Connecting);
        assert_eq!(ch.status(), ChannelStatus::Connecting);

        ch.set_status(ChannelStatus::Connected);
        assert_eq!(ch.status(), ChannelStatus::Connected);

        ch.set_status(ChannelStatus::Error);
        assert_eq!(ch.status(), ChannelStatus::Error);
    }
}
