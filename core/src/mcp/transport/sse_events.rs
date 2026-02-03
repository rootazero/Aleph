//! SSE Event Types
//!
//! Types for parsing Server-Sent Events from MCP servers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// SSE event types from MCP server
#[derive(Debug, Clone)]
pub enum SseEvent {
    /// JSON-RPC notification from server
    Notification(SseNotification),
    /// JSON-RPC request from server (sampling, etc.)
    Request(SseRequest),
    /// Endpoint message (server telling client where to POST)
    Endpoint { url: String },
    /// Ping/keepalive
    Ping,
    /// Unknown event type
    Unknown { event_type: String, data: String },
}

/// Server notification via SSE
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseNotification {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Method name
    pub method: String,
    /// Parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Server request via SSE (bidirectional RPC)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseRequest {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request ID for response correlation
    pub id: u64,
    /// Method name
    pub method: String,
    /// Parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl SseEvent {
    /// Parse SSE event from event type and data
    pub fn parse(event_type: &str, data: &str) -> Self {
        match event_type {
            "endpoint" => {
                SseEvent::Endpoint { url: data.trim().to_string() }
            }
            "ping" => SseEvent::Ping,
            "message" | "" => {
                // Try to parse as JSON-RPC
                if let Ok(value) = serde_json::from_str::<Value>(data) {
                    // Check if it's a request (has id) or notification (no id)
                    if value.get("id").is_some() && value.get("method").is_some() {
                        if let Ok(req) = serde_json::from_value(value) {
                            return SseEvent::Request(req);
                        }
                    } else if value.get("method").is_some() {
                        if let Ok(notif) = serde_json::from_value(value) {
                            return SseEvent::Notification(notif);
                        }
                    }
                }
                SseEvent::Unknown {
                    event_type: event_type.to_string(),
                    data: data.to_string(),
                }
            }
            _ => SseEvent::Unknown {
                event_type: event_type.to_string(),
                data: data.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_notification() {
        let data = r#"{"jsonrpc":"2.0","method":"notifications/tools/listChanged"}"#;
        let event = SseEvent::parse("message", data);
        assert!(matches!(event, SseEvent::Notification(_)));
        if let SseEvent::Notification(n) = event {
            assert_eq!(n.method, "notifications/tools/listChanged");
        }
    }

    #[test]
    fn test_parse_request() {
        let data = r#"{"jsonrpc":"2.0","id":1,"method":"sampling/createMessage","params":{}}"#;
        let event = SseEvent::parse("message", data);
        assert!(matches!(event, SseEvent::Request(_)));
        if let SseEvent::Request(r) = event {
            assert_eq!(r.id, 1);
            assert_eq!(r.method, "sampling/createMessage");
        }
    }

    #[test]
    fn test_parse_endpoint() {
        let event = SseEvent::parse("endpoint", "https://example.com/mcp");
        if let SseEvent::Endpoint { url } = event {
            assert_eq!(url, "https://example.com/mcp");
        } else {
            panic!("Expected Endpoint event");
        }
    }

    #[test]
    fn test_parse_ping() {
        let event = SseEvent::parse("ping", "");
        assert!(matches!(event, SseEvent::Ping));
    }

    #[test]
    fn test_parse_unknown() {
        let event = SseEvent::parse("custom", "some data");
        assert!(matches!(event, SseEvent::Unknown { .. }));
    }

    #[test]
    fn test_parse_empty_event_type() {
        // Empty event type with JSON-RPC data should still parse
        let data = r#"{"jsonrpc":"2.0","method":"test"}"#;
        let event = SseEvent::parse("", data);
        assert!(matches!(event, SseEvent::Notification(_)));
    }
}
