//! Bridge RPC Protocol Types
//!
//! JSON-RPC 2.0 message types for communication between the Rust aleph
//! and the Go whatsapp-bridge binary over Unix domain socket (newline-delimited JSON).
//!
//! # Wire Protocol
//!
//! - Transport: Unix domain socket
//! - Framing: newline-delimited JSON (one JSON object per line)
//! - Protocol: JSON-RPC 2.0
//!
//! # Direction
//!
//! - **Request types** (Rust -> Go): `Serialize`
//! - **Response types** (Go -> Rust): `Deserialize`
//! - **BridgeEvent** (Go -> Rust push): `Deserialize + Clone`
//! - **MediaPayload**: `Serialize + Deserialize` (used in both directions)

use serde::{Deserialize, Serialize};

// ─── Request Types (Rust → Go) ───────────────────────────────────────────────

/// Request the bridge to connect to WhatsApp servers.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectRequest {}

/// Request the bridge to disconnect from WhatsApp servers.
#[derive(Debug, Clone, Serialize)]
pub struct DisconnectRequest {}

/// Request the bridge to send a message.
#[derive(Debug, Clone, Serialize)]
pub struct SendRequest {
    /// Recipient JID (e.g. "1234567890@s.whatsapp.net")
    pub to: String,
    /// Text content of the message
    pub text: String,
    /// Optional media attachment
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaPayload>,
    /// Optional message ID to reply to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// Media attachment payload, used in both send requests and inbound messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaPayload {
    /// MIME type of the media (e.g. "image/jpeg", "application/pdf")
    pub mime_type: String,
    /// Base64-encoded media data
    pub data: String,
    /// Optional filename for the attachment
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Ping the bridge to check liveness.
#[derive(Debug, Clone, Serialize)]
pub struct PingRequest {}

/// Request bridge status information.
#[derive(Debug, Clone, Serialize)]
pub struct StatusRequest {}

// ─── Response Types (Go → Rust) ──────────────────────────────────────────────

/// Simple acknowledgment response.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OkResponse {
    /// Whether the operation succeeded
    pub ok: bool,
}

/// Response to a send request, containing the server-assigned message ID.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SendResponse {
    /// Server-assigned message ID
    pub id: String,
}

/// Response to a ping request.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PingResponse {
    /// Whether the bridge responded to the ping
    pub pong: bool,
    /// Round-trip time in milliseconds (if measured)
    pub rtt_ms: Option<u64>,
}

/// Response to a status request with bridge connection details.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BridgeStatusResponse {
    /// Whether the bridge is connected to WhatsApp
    pub connected: bool,
    /// Name of the linked device
    pub device_name: Option<String>,
    /// Phone number associated with the linked account
    pub phone_number: Option<String>,
}

// ─── Event Types (Go → Rust push notifications) ─────────────────────────────

/// Events pushed from the Go bridge to the Rust server.
///
/// These are not responses to requests; they arrive asynchronously
/// as WhatsApp state changes or messages are received.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// QR code available for scanning to pair a device.
    Qr {
        /// QR code data (to be rendered as a QR image)
        qr_data: String,
        /// Seconds until this QR code expires
        expires_in_secs: u64,
    },

    /// The QR code has expired and a new one must be requested.
    QrExpired,

    /// The QR code has been scanned by the user's phone.
    Scanned,

    /// Initial history sync is in progress.
    Syncing {
        /// Sync progress as a fraction (0.0 to 1.0)
        progress: f32,
    },

    /// Successfully connected to WhatsApp.
    Connected {
        /// Name of the linked device
        device_name: String,
        /// Phone number of the linked account
        phone_number: String,
    },

    /// Disconnected from WhatsApp.
    Disconnected {
        /// Reason for disconnection
        reason: String,
    },

    /// An inbound WhatsApp message.
    Message {
        /// Sender JID
        from: String,
        /// Sender display name (if available)
        from_name: Option<String>,
        /// Chat/conversation JID
        chat_id: String,
        /// Text content of the message
        text: String,
        /// Optional media attachment
        media: Option<MediaPayload>,
        /// Unix timestamp of the message
        timestamp: i64,
        /// Server-assigned message ID
        message_id: String,
        /// Whether this message is from a group chat
        is_group: bool,
        /// Message ID this is replying to (if a reply)
        reply_to: Option<String>,
    },

    /// A delivery/read receipt for a previously sent message.
    Receipt {
        /// ID of the message this receipt is for
        message_id: String,
        /// Receipt type (e.g. "delivered", "read", "played")
        receipt_type: String,
    },

    /// The bridge is fully initialized and ready to send/receive.
    Ready,

    /// An error occurred in the bridge.
    Error {
        /// Human-readable error description
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── SendRequest serialization tests ─────────────────────────────

    #[test]
    fn test_send_request_simple() {
        let req = SendRequest {
            to: "1234567890@s.whatsapp.net".to_string(),
            text: "Hello, world!".to_string(),
            media: None,
            reply_to: None,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["to"], "1234567890@s.whatsapp.net");
        assert_eq!(json["text"], "Hello, world!");
        // None fields should be omitted via skip_serializing_if
        assert!(json.get("media").is_none());
        assert!(json.get("reply_to").is_none());
    }

    #[test]
    fn test_send_request_with_media() {
        let req = SendRequest {
            to: "1234567890@s.whatsapp.net".to_string(),
            text: "Check this out".to_string(),
            media: Some(MediaPayload {
                mime_type: "image/jpeg".to_string(),
                data: "base64encodeddata==".to_string(),
                filename: Some("photo.jpg".to_string()),
            }),
            reply_to: Some("msg-abc-123".to_string()),
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["to"], "1234567890@s.whatsapp.net");
        assert_eq!(json["text"], "Check this out");
        assert_eq!(json["media"]["mime_type"], "image/jpeg");
        assert_eq!(json["media"]["data"], "base64encodeddata==");
        assert_eq!(json["media"]["filename"], "photo.jpg");
        assert_eq!(json["reply_to"], "msg-abc-123");
    }

    #[test]
    fn test_send_request_skip_serializing_none_fields() {
        let req = SendRequest {
            to: "user@s.whatsapp.net".to_string(),
            text: "hi".to_string(),
            media: None,
            reply_to: None,
        };

        let json_str = serde_json::to_string(&req).unwrap();
        assert!(!json_str.contains("media"));
        assert!(!json_str.contains("reply_to"));
    }

    #[test]
    fn test_media_payload_without_filename() {
        let media = MediaPayload {
            mime_type: "audio/ogg".to_string(),
            data: "audiodata==".to_string(),
            filename: None,
        };

        let json_str = serde_json::to_string(&media).unwrap();
        assert!(json_str.contains("audio/ogg"));
        assert!(!json_str.contains("filename"));
    }

    #[test]
    fn test_connect_request_serialization() {
        let req = ConnectRequest {};
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.is_object());
        assert_eq!(json.as_object().unwrap().len(), 0);
    }

    #[test]
    fn test_disconnect_request_serialization() {
        let req = DisconnectRequest {};
        let json_str = serde_json::to_string(&req).unwrap();
        assert_eq!(json_str, "{}");
    }

    #[test]
    fn test_ping_request_serialization() {
        let req = PingRequest {};
        let json_str = serde_json::to_string(&req).unwrap();
        assert_eq!(json_str, "{}");
    }

    #[test]
    fn test_status_request_serialization() {
        let req = StatusRequest {};
        let json_str = serde_json::to_string(&req).unwrap();
        assert_eq!(json_str, "{}");
    }

    // ─── Response deserialization tests ──────────────────────────────

    #[test]
    fn test_ok_response_deserialization() {
        let json = r#"{"ok": true}"#;
        let resp: OkResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);

        let json = r#"{"ok": false}"#;
        let resp: OkResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
    }

    #[test]
    fn test_send_response_deserialization() {
        let json = r#"{"id": "3EB0A1B2C3D4E5F6"}"#;
        let resp: SendResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "3EB0A1B2C3D4E5F6");
    }

    #[test]
    fn test_ping_response_deserialization() {
        let json = r#"{"pong": true, "rtt_ms": 42}"#;
        let resp: PingResponse = serde_json::from_str(json).unwrap();
        assert!(resp.pong);
        assert_eq!(resp.rtt_ms, Some(42));
    }

    #[test]
    fn test_ping_response_without_rtt() {
        let json = r#"{"pong": true}"#;
        let resp: PingResponse = serde_json::from_str(json).unwrap();
        assert!(resp.pong);
        assert_eq!(resp.rtt_ms, None);
    }

    #[test]
    fn test_bridge_status_response_connected() {
        let json = r#"{
            "connected": true,
            "device_name": "iPhone 15",
            "phone_number": "+1234567890"
        }"#;
        let resp: BridgeStatusResponse = serde_json::from_str(json).unwrap();
        assert!(resp.connected);
        assert_eq!(resp.device_name, Some("iPhone 15".to_string()));
        assert_eq!(resp.phone_number, Some("+1234567890".to_string()));
    }

    #[test]
    fn test_bridge_status_response_disconnected() {
        let json = r#"{"connected": false}"#;
        let resp: BridgeStatusResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.connected);
        assert_eq!(resp.device_name, None);
        assert_eq!(resp.phone_number, None);
    }

    // ─── BridgeEvent deserialization tests ───────────────────────────

    #[test]
    fn test_event_qr() {
        let json = r#"{"type": "qr", "qr_data": "2@ABC123", "expires_in_secs": 60}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Qr { qr_data, expires_in_secs } => {
                assert_eq!(qr_data, "2@ABC123");
                assert_eq!(expires_in_secs, 60);
            }
            _ => panic!("Expected Qr event"),
        }
    }

    #[test]
    fn test_event_qr_expired() {
        let json = r#"{"type": "qr_expired"}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event, BridgeEvent::QrExpired);
    }

    #[test]
    fn test_event_scanned() {
        let json = r#"{"type": "scanned"}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event, BridgeEvent::Scanned);
    }

    #[test]
    fn test_event_syncing() {
        let json = r#"{"type": "syncing", "progress": 0.75}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Syncing { progress } => {
                assert!((progress - 0.75).abs() < f32::EPSILON);
            }
            _ => panic!("Expected Syncing event"),
        }
    }

    #[test]
    fn test_event_connected() {
        let json = r#"{
            "type": "connected",
            "device_name": "Pixel 8",
            "phone_number": "+8613800138000"
        }"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Connected { device_name, phone_number } => {
                assert_eq!(device_name, "Pixel 8");
                assert_eq!(phone_number, "+8613800138000");
            }
            _ => panic!("Expected Connected event"),
        }
    }

    #[test]
    fn test_event_disconnected() {
        let json = r#"{"type": "disconnected", "reason": "logged out from phone"}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Disconnected { reason } => {
                assert_eq!(reason, "logged out from phone");
            }
            _ => panic!("Expected Disconnected event"),
        }
    }

    #[test]
    fn test_event_message_simple() {
        let json = r#"{
            "type": "message",
            "from": "1234567890@s.whatsapp.net",
            "from_name": "Alice",
            "chat_id": "1234567890@s.whatsapp.net",
            "text": "Hello!",
            "media": null,
            "timestamp": 1708531200,
            "message_id": "3EB0A1B2C3D4",
            "is_group": false,
            "reply_to": null
        }"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Message {
                from,
                from_name,
                chat_id,
                text,
                media,
                timestamp,
                message_id,
                is_group,
                reply_to,
            } => {
                assert_eq!(from, "1234567890@s.whatsapp.net");
                assert_eq!(from_name, Some("Alice".to_string()));
                assert_eq!(chat_id, "1234567890@s.whatsapp.net");
                assert_eq!(text, "Hello!");
                assert!(media.is_none());
                assert_eq!(timestamp, 1708531200);
                assert_eq!(message_id, "3EB0A1B2C3D4");
                assert!(!is_group);
                assert!(reply_to.is_none());
            }
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_event_message_with_media() {
        let json = r#"{
            "type": "message",
            "from": "group@g.us",
            "from_name": null,
            "chat_id": "group@g.us",
            "text": "",
            "media": {
                "mime_type": "image/png",
                "data": "iVBORw0KGgo=",
                "filename": "screenshot.png"
            },
            "timestamp": 1708531300,
            "message_id": "msg-xyz",
            "is_group": true,
            "reply_to": "msg-abc"
        }"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Message {
                from,
                media,
                is_group,
                reply_to,
                ..
            } => {
                assert_eq!(from, "group@g.us");
                assert!(is_group);
                assert_eq!(reply_to, Some("msg-abc".to_string()));
                let media = media.unwrap();
                assert_eq!(media.mime_type, "image/png");
                assert_eq!(media.data, "iVBORw0KGgo=");
                assert_eq!(media.filename, Some("screenshot.png".to_string()));
            }
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_event_message_minimal_fields() {
        // Test with optional fields omitted entirely (not null)
        let json = r#"{
            "type": "message",
            "from": "user@s.whatsapp.net",
            "chat_id": "user@s.whatsapp.net",
            "text": "hi",
            "timestamp": 1708531400,
            "message_id": "msg-001",
            "is_group": false
        }"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Message {
                from_name,
                media,
                reply_to,
                ..
            } => {
                assert!(from_name.is_none());
                assert!(media.is_none());
                assert!(reply_to.is_none());
            }
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_event_receipt() {
        let json = r#"{
            "type": "receipt",
            "message_id": "3EB0A1B2C3D4",
            "receipt_type": "read"
        }"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Receipt { message_id, receipt_type } => {
                assert_eq!(message_id, "3EB0A1B2C3D4");
                assert_eq!(receipt_type, "read");
            }
            _ => panic!("Expected Receipt event"),
        }
    }

    #[test]
    fn test_event_ready() {
        let json = r#"{"type": "ready"}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event, BridgeEvent::Ready);
    }

    #[test]
    fn test_event_error() {
        let json = r#"{"type": "error", "message": "connection timeout"}"#;
        let event: BridgeEvent = serde_json::from_str(json).unwrap();
        match event {
            BridgeEvent::Error { message } => {
                assert_eq!(message, "connection timeout");
            }
            _ => panic!("Expected Error event"),
        }
    }

    // ─── Roundtrip serialization tests ──────────────────────────────

    #[test]
    fn test_media_payload_roundtrip() {
        let original = MediaPayload {
            mime_type: "application/pdf".to_string(),
            data: "JVBERi0xLjQ=".to_string(),
            filename: Some("document.pdf".to_string()),
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: MediaPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_media_payload_roundtrip_no_filename() {
        let original = MediaPayload {
            mime_type: "image/webp".to_string(),
            data: "UklGR...".to_string(),
            filename: None,
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: MediaPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_send_request_json_structure() {
        // Verify the exact JSON structure that the Go bridge expects
        let req = SendRequest {
            to: "user@s.whatsapp.net".to_string(),
            text: "Hello".to_string(),
            media: Some(MediaPayload {
                mime_type: "text/plain".to_string(),
                data: "SGVsbG8=".to_string(),
                filename: Some("hello.txt".to_string()),
            }),
            reply_to: Some("reply-id".to_string()),
        };

        let json = serde_json::to_value(&req).unwrap();
        let obj = json.as_object().unwrap();

        // Verify all expected keys are present
        assert!(obj.contains_key("to"));
        assert!(obj.contains_key("text"));
        assert!(obj.contains_key("media"));
        assert!(obj.contains_key("reply_to"));

        // Verify media sub-structure
        let media = obj["media"].as_object().unwrap();
        assert!(media.contains_key("mime_type"));
        assert!(media.contains_key("data"));
        assert!(media.contains_key("filename"));
    }

    #[test]
    fn test_bridge_event_clone() {
        // Verify Clone is properly derived on BridgeEvent
        let event = BridgeEvent::Connected {
            device_name: "Test Device".to_string(),
            phone_number: "+1234567890".to_string(),
        };
        let cloned = event.clone();
        assert_eq!(event, cloned);
    }

    #[test]
    fn test_receipt_types() {
        // Test various receipt types that WhatsApp supports
        for receipt_type in &["delivered", "read", "played"] {
            let json = format!(
                r#"{{"type": "receipt", "message_id": "msg-1", "receipt_type": "{}"}}"#,
                receipt_type
            );
            let event: BridgeEvent = serde_json::from_str(&json).unwrap();
            match event {
                BridgeEvent::Receipt { receipt_type: rt, .. } => {
                    assert_eq!(rt, *receipt_type);
                }
                _ => panic!("Expected Receipt event"),
            }
        }
    }
}
