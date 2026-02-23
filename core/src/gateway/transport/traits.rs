//! Transport trait abstraction for bridge process communication.
//!
//! Defines a platform-independent IPC interface that bridge processes
//! (Signal, WhatsApp, Telegram, etc.) implement to communicate with
//! the Aleph gateway. The transport layer handles JSON-RPC 2.0
//! framing over various IPC mechanisms (Unix sockets, stdio, etc.).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Bridge events (platform-independent)
// ---------------------------------------------------------------------------

/// Standardized bridge events emitted by external bridge processes.
///
/// All bridge implementations translate their native events into this
/// common format before forwarding them to the gateway. The enum is
/// tagged by `"type"` in JSON so the gateway can dispatch without
/// peeking into variant-specific fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// Bridge has finished initialization and is ready to accept commands.
    Ready,

    /// Bridge status changed (e.g. "connecting", "connected", "disconnected").
    StatusChange {
        status: String,
    },

    /// Pairing flow progress update (QR code, scan, sync, etc.).
    PairingUpdate(PairingEvent),

    /// An inbound message from the external platform.
    Message {
        /// Platform-specific sender identifier.
        from: String,
        /// Human-readable sender name, if available.
        #[serde(default)]
        sender_name: Option<String>,
        /// Conversation / chat identifier.
        conversation_id: String,
        /// Message text body.
        text: String,
        /// Platform-specific message identifier.
        message_id: String,
        /// Unix timestamp in seconds.
        timestamp: i64,
        /// Whether the message comes from a group conversation.
        is_group: bool,
        /// Optional file attachments (images, voice notes, etc.).
        #[serde(default)]
        attachments: Vec<AttachmentPayload>,
        /// If this message is a reply, the original message id.
        #[serde(default)]
        reply_to: Option<String>,
    },

    /// Delivery / read receipt for a previously sent message.
    Receipt {
        message_id: String,
        /// "delivered", "read", "played", etc.
        receipt_type: String,
    },

    /// An error reported by the bridge process.
    Error {
        message: String,
    },
}

/// Pairing flow events, tagged by `"phase"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PairingEvent {
    /// A QR code is available for scanning.
    QrCode {
        qr_data: String,
        expires_in_secs: u64,
    },
    /// The previous QR code has expired; a new one will follow.
    QrExpired,
    /// The QR code was scanned by the user's device.
    Scanned,
    /// Initial history / contact sync is in progress.
    Syncing {
        /// 0.0 .. 1.0
        progress: f32,
    },
    /// Pairing completed successfully.
    Connected {
        device_name: String,
        identifier: String,
    },
    /// Pairing failed with an error.
    Failed {
        error: String,
    },
}

/// A file attachment carried within a [`BridgeEvent::Message`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentPayload {
    /// MIME type (e.g. "image/png", "audio/ogg").
    pub mime_type: String,
    /// Base64-encoded file data.
    pub data: String,
    /// Original filename, if known.
    pub filename: Option<String>,
}

// ---------------------------------------------------------------------------
// Transport error
// ---------------------------------------------------------------------------

/// Errors that can occur during transport operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Request failed: {0}")]
    RequestFailed(String),

    #[error("Request timed out")]
    Timeout,

    #[error("Not connected")]
    NotConnected,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

// ---------------------------------------------------------------------------
// Transport trait
// ---------------------------------------------------------------------------

/// Abstract IPC transport for communicating with external bridge processes.
///
/// Implementations handle the specifics of the IPC mechanism (Unix socket,
/// stdio pipes, TCP, etc.) while exposing a uniform request/event interface
/// to the rest of the gateway.
///
/// # Object Safety
///
/// The trait is object-safe so it can be stored as `Box<dyn Transport>`.
#[async_trait]
pub trait Transport: Send + Sync + fmt::Debug {
    /// Send a JSON-RPC 2.0 request and wait for the matching response.
    ///
    /// The transport assigns a unique request id internally and correlates
    /// the response. Returns the `result` field on success or a
    /// [`TransportError`] on failure / timeout.
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError>;

    /// Receive the next event notification from the bridge.
    ///
    /// Returns `None` when the transport is closed or the bridge has
    /// disconnected.
    ///
    /// Note: Only a single concurrent caller is supported. If multiple tasks
    /// call `next_event` simultaneously, only one will receive each event.
    async fn next_event(&self) -> Option<BridgeEvent>;

    /// Gracefully close the transport connection.
    async fn close(&self) -> Result<(), TransportError>;

    /// Returns `true` if the transport is currently connected.
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The Transport trait must be object-safe so we can use `dyn Transport`.
    #[test]
    fn _assert_object_safe() {
        fn _check(_t: &dyn Transport) {}
    }

    #[test]
    fn test_bridge_event_variants() {
        // Ensure all variants can be constructed without panicking.
        let _ready = BridgeEvent::Ready;

        let _status = BridgeEvent::StatusChange {
            status: "connected".into(),
        };

        let _pairing = BridgeEvent::PairingUpdate(PairingEvent::QrCode {
            qr_data: "data".into(),
            expires_in_secs: 60,
        });

        let _message = BridgeEvent::Message {
            from: "+1234567890".into(),
            sender_name: Some("Alice".into()),
            conversation_id: "conv-1".into(),
            text: "Hello".into(),
            message_id: "msg-1".into(),
            timestamp: 1700000000,
            is_group: false,
            attachments: vec![AttachmentPayload {
                mime_type: "image/png".into(),
                data: "base64data".into(),
                filename: Some("photo.png".into()),
            }],
            reply_to: None,
        };

        let _receipt = BridgeEvent::Receipt {
            message_id: "msg-1".into(),
            receipt_type: "read".into(),
        };

        let _error = BridgeEvent::Error {
            message: "something went wrong".into(),
        };
    }

    #[test]
    fn test_pairing_event_variants() {
        let _qr = PairingEvent::QrCode {
            qr_data: "qr-payload".into(),
            expires_in_secs: 120,
        };
        let _expired = PairingEvent::QrExpired;
        let _scanned = PairingEvent::Scanned;
        let _syncing = PairingEvent::Syncing { progress: 0.5 };
        let _connected = PairingEvent::Connected {
            device_name: "iPhone".into(),
            identifier: "id-123".into(),
        };
        let _failed = PairingEvent::Failed {
            error: "timeout".into(),
        };
    }

    #[test]
    fn test_bridge_event_serialization() {
        // Round-trip: serialize then deserialize each variant.
        let events = vec![
            BridgeEvent::Ready,
            BridgeEvent::StatusChange {
                status: "connecting".into(),
            },
            BridgeEvent::PairingUpdate(PairingEvent::QrCode {
                qr_data: "qr".into(),
                expires_in_secs: 30,
            }),
            BridgeEvent::Message {
                from: "user@example.com".into(),
                sender_name: None,
                conversation_id: "c1".into(),
                text: "hi".into(),
                message_id: "m1".into(),
                timestamp: 1234567890,
                is_group: true,
                attachments: vec![],
                reply_to: Some("m0".into()),
            },
            BridgeEvent::Receipt {
                message_id: "m1".into(),
                receipt_type: "delivered".into(),
            },
            BridgeEvent::Error {
                message: "oops".into(),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).expect("serialize");
            let back: BridgeEvent = serde_json::from_str(&json).expect("deserialize");

            // Verify the round-trip preserves the discriminant tag.
            let json_orig: serde_json::Value =
                serde_json::from_str(&json).expect("parse original");
            let json_back: serde_json::Value =
                serde_json::to_value(&back).expect("re-serialize");
            assert_eq!(json_orig, json_back, "round-trip mismatch for: {json}");
        }
    }

    #[test]
    fn test_pairing_event_serialization() {
        let events = vec![
            PairingEvent::QrCode {
                qr_data: "data".into(),
                expires_in_secs: 60,
            },
            PairingEvent::QrExpired,
            PairingEvent::Scanned,
            PairingEvent::Syncing { progress: 0.75 },
            PairingEvent::Connected {
                device_name: "Pixel".into(),
                identifier: "abc".into(),
            },
            PairingEvent::Failed {
                error: "denied".into(),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).expect("serialize");
            let back: PairingEvent = serde_json::from_str(&json).expect("deserialize");
            let json_orig: serde_json::Value =
                serde_json::from_str(&json).expect("parse original");
            let json_back: serde_json::Value =
                serde_json::to_value(&back).expect("re-serialize");
            assert_eq!(json_orig, json_back, "round-trip mismatch for: {json}");
        }
    }

    #[test]
    fn test_attachment_payload_serialization() {
        let attachment = AttachmentPayload {
            mime_type: "audio/ogg".into(),
            data: "base64==".into(),
            filename: Some("voice.ogg".into()),
        };
        let json = serde_json::to_string(&attachment).unwrap();
        let back: AttachmentPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mime_type, "audio/ogg");
        assert_eq!(back.filename.as_deref(), Some("voice.ogg"));
    }

    #[test]
    fn test_transport_error_display() {
        let err = TransportError::ConnectionFailed("refused".into());
        assert_eq!(err.to_string(), "Connection failed: refused");

        let err = TransportError::Timeout;
        assert_eq!(err.to_string(), "Request timed out");

        let err = TransportError::NotConnected;
        assert_eq!(err.to_string(), "Not connected");
    }
}
