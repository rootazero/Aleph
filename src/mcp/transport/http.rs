//! Streamable HTTP Transport for Remote MCP Servers
//!
//! Implements MCP communication over HTTP POST per the Streamable HTTP
//! transport (spec revision 2025-03-26), while remaining compatible with
//! plain JSON-RPC-over-POST servers:
//!
//! - Every request advertises `Accept: application/json, text/event-stream`.
//!   The spec requires the client to list both; official SDK servers reject
//!   requests without it.
//! - The `Mcp-Session-Id` response header is captured (servers assign it on
//!   `initialize`) and echoed on every subsequent request and notification.
//!   Stateful SDK servers reject session-less follow-up requests outright,
//!   which previously made every such server unusable from Aleph.
//! - POST responses delivered as `text/event-stream` are scanned for the
//!   JSON-RPC response matching the request id (servers may interleave
//!   notifications on the same stream).
//! - `404` on a session-bearing request means the server expired the session;
//!   the stored id is cleared so the manager's health/restart cycle can
//!   re-initialize cleanly.
//! - `close()` sends a best-effort `DELETE` to terminate the session.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::protocol::MCP_PROTOCOL_VERSION;
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SafeFetchResponse, SsrfPolicy};

/// Header carrying the Streamable HTTP session identifier.
const SESSION_HEADER: &str = "Mcp-Session-Id";

/// HTTP transport configuration
#[derive(Debug, Clone)]
pub struct HttpTransportConfig {
    /// Server URL (e.g., "<https://example.com/mcp>")
    pub url: String,
    /// Custom HTTP headers (for auth tokens, etc.)
    pub headers: HashMap<String, String>,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for HttpTransportConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        }
    }
}

/// Streamable HTTP transport for remote MCP servers
///
/// Each JSON-RPC request is sent as a POST to the configured URL; the
/// response arrives either as a plain JSON body or as an SSE stream that is
/// drained for the matching response message.
///
/// # Limitations
///
/// - The optional server-initiated GET stream is not opened, so this
///   transport does not receive unsolicited server notifications; use the
///   SSE transport for servers that push (sampling, list-changed).
pub struct HttpTransport {
    /// Server name for logging
    server_name: String,
    /// Configuration
    config: HttpTransportConfig,
    /// Connection state
    alive: RwLock<bool>,
    /// Streamable HTTP session id assigned by the server on `initialize`
    session_id: RwLock<Option<String>>,
    /// Protocol version negotiated on `initialize`. Once set, it is echoed on
    /// the `MCP-Protocol-Version` header instead of our proposed default, so a
    /// server that negotiated an older revision receives the value it chose.
    /// A `std` lock (not tokio's) because it is written from the sync
    /// `set_protocol_version` trait method and only held to clone a small string.
    negotiated_version: std::sync::RwLock<Option<String>>,
    /// Notification handler (stored but not actively used in HTTP transport)
    _notification_handler: RwLock<Option<NotificationCallback>>,
}

impl HttpTransport {
    /// Create a new HTTP transport
    ///
    /// # Arguments
    ///
    /// * `name` - Server name for logging and identification
    /// * `config` - HTTP transport configuration
    ///
    pub fn new(name: impl Into<String>, config: HttpTransportConfig) -> Result<Self> {
        Ok(Self {
            server_name: name.into(),
            config,
            alive: RwLock::new(true),
            session_id: RwLock::new(None),
            negotiated_version: std::sync::RwLock::new(None),
            _notification_handler: RwLock::new(None),
        })
    }

    async fn request_headers(&self, session: Option<&str>) -> Result<HeaderMap> {
        let version_guard = self
            .negotiated_version
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let protocol_version = version_guard.as_deref().unwrap_or(MCP_PROTOCOL_VERSION);

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_str(protocol_version)
                .map_err(|e| AlephError::IoError(format!("Invalid MCP protocol version: {e}")))?,
        );
        if let Some(session) = session {
            headers.insert(
                HeaderName::from_static("mcp-session-id"),
                HeaderValue::from_str(session)
                    .map_err(|e| AlephError::IoError(format!("Invalid MCP session id: {e}")))?,
            );
        }
        for (key, value) in &self.config.headers {
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|e| AlephError::IoError(format!("Invalid MCP header name: {e}")))?;
            let value = HeaderValue::from_str(value)
                .map_err(|e| AlephError::IoError(format!("Invalid MCP header value: {e}")))?;
            headers.insert(name, value);
        }
        Ok(headers)
    }

    async fn send_body(&self, body: Vec<u8>, session: Option<&str>) -> Result<SafeFetchResponse> {
        let headers = self.request_headers(session).await?;
        safe_fetch(
            &self.config.url,
            &SsrfPolicy::default(),
            SafeFetchRequest::post(body, self.config.timeout).with_headers(headers),
        )
        .await
        .map_err(|e| {
            AlephError::IoError(format!(
                "HTTP request to '{}' failed: {e}",
                self.server_name
            ))
        })
    }

    /// Capture the session id from a response, if the server assigned one
    async fn capture_session(&self, headers: &HeaderMap) {
        let value = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
        if let Some(value) = value {
            let mut session = self.session_id.write().await;
            if session.as_deref() != Some(value) {
                tracing::debug!(server = %self.server_name, "Captured MCP session id");
                *session = Some(value.to_string());
            }
        }
    }

    /// Clear an expired session. Returns true if one was active.
    async fn clear_expired_session(&self) -> bool {
        let mut session = self.session_id.write().await;
        session.take().is_some()
    }
}

/// Extract the JSON-RPC response with `expected_id` from an SSE-formatted
/// body.
///
/// Servers may interleave notifications or their own requests on the same
/// stream; a stray `{jsonrpc, method, ...}` frame would still satisfy
/// `JsonRpcResponse`'s optional fields, so a frame only counts when it
/// carries our id AND a `result`/`error` member.
fn parse_sse_response(body: &str, expected_id: u64) -> Option<JsonRpcResponse> {
    let mut data = String::new();
    let mut found: Option<JsonRpcResponse> = None;

    fn flush(data: &mut String, found: &mut Option<JsonRpcResponse>, expected_id: u64) {
        if data.is_empty() {
            return;
        }
        if found.is_none() {
            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(data) {
                if resp.id == Some(expected_id) && (resp.result.is_some() || resp.error.is_some()) {
                    *found = Some(resp);
                }
            }
        }
        data.clear();
    }

    for line in body.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            flush(&mut data, &mut found, expected_id);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // Other SSE fields (event:, id:, retry:, comments) carry no payload.
    }
    flush(&mut data, &mut found, expected_id);
    found
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let body = serde_json::to_vec(request)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize request: {e}")))?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending HTTP request"
        );

        let session = self.session_id.read().await.clone();
        let response = self.send_body(body, session.as_deref()).await?;
        let status = response.status;

        if status == reqwest::StatusCode::NOT_FOUND && self.clear_expired_session().await {
            return Err(AlephError::IoError(format!(
                "MCP session for '{}' expired (HTTP 404); server requires re-initialization",
                self.server_name
            )));
        }

        if !status.is_success() {
            let body = String::from_utf8_lossy(&response.body);
            return Err(AlephError::IoError(format!(
                "HTTP {} from '{}': {}",
                status, self.server_name, body
            )));
        }

        self.capture_session(&response.headers).await;

        let is_sse = response
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));

        let text = String::from_utf8(response.body)
            .map_err(|e| AlephError::IoError(format!("Failed to read response: {e}")))?;

        if is_sse {
            return parse_sse_response(&text, request.id).ok_or_else(|| {
                AlephError::IoError(format!(
                    "SSE response stream from '{}' ended without a response to request {}",
                    self.server_name, request.id
                ))
            });
        }

        serde_json::from_str(&text).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to parse response from '{}': {} (body: {})",
                self.server_name, e, text
            ))
        })
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        let body = serde_json::to_vec(notification)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize notification: {e}")))?;

        tracing::debug!(
            server = %self.server_name,
            method = %notification.method,
            "Sending HTTP notification"
        );

        let session = self.session_id.read().await.clone();
        let response = self.send_body(body, session.as_deref()).await?;
        let status = response.status;

        if status.is_success() {
            self.capture_session(&response.headers).await;
        } else {
            if status == reqwest::StatusCode::NOT_FOUND {
                self.clear_expired_session().await;
            }
            tracing::warn!(
                server = %self.server_name,
                status = %status,
                "HTTP notification returned non-success status"
            );
        }

        Ok(())
    }

    async fn is_alive(&self) -> bool {
        *self.alive.read().await
    }

    async fn close(&self) -> Result<()> {
        let session = self.session_id.write().await.take();
        if let Some(session) = session {
            match self.request_headers(Some(&session)).await {
                Ok(headers) => {
                    let request = SafeFetchRequest::get(self.config.timeout)
                        .with_method(Method::DELETE)
                        .with_headers(headers);
                    if let Err(e) =
                        safe_fetch(&self.config.url, &SsrfPolicy::default(), request).await
                    {
                        tracing::debug!(
                            server = %self.server_name,
                            error = %e,
                            "MCP session DELETE failed (best-effort)"
                        );
                    }
                }
                Err(e) => tracing::debug!(
                    server = %self.server_name,
                    error = %e,
                    "MCP session DELETE header construction failed (best-effort)"
                ),
            }
        }

        let mut alive = self.alive.write().await;
        *alive = false;
        Ok(())
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }

    fn set_notification_handler(&self, handler: NotificationCallback) {
        // HTTP transport doesn't support server-initiated notifications
        // in basic mode, but we store it for potential polling implementation
        tracing::debug!(
            server = %self.server_name,
            "Notification handler set (HTTP transport has limited notification support)"
        );
        // Could implement polling here in the future
        let _ = handler; // Acknowledge but don't use
    }

    fn set_protocol_version(&self, version: &str) {
        let mut slot = self
            .negotiated_version
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if slot.as_deref() != Some(version) {
            tracing::debug!(server = %self.server_name, version, "Captured negotiated MCP protocol version");
            *slot = Some(version.to_string());
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_transport_config() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        assert_eq!(config.url, "https://example.com/mcp");
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_http_transport_config_default() {
        let config = HttpTransportConfig::default();

        assert!(config.url.is_empty());
        assert!(config.headers.is_empty());
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_http_transport_creation() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        let transport = HttpTransport::new("test-server", config).unwrap();
        assert_eq!(transport.server_name(), "test-server");
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_http_transport_close() {
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new("test", config).unwrap();

        assert!(transport.is_alive().await);
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[test]
    fn test_http_transport_config_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());

        let config = HttpTransportConfig {
            url: "https://api.example.com/mcp".to_string(),
            headers,
            timeout: Duration::from_secs(60),
        };

        assert!(config.headers.contains_key("Authorization"));
        assert!(config.headers.contains_key("X-Custom-Header"));
        assert_eq!(config.timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_http_transport_implements_mcp_transport_trait() {
        let config = HttpTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        // Verify it can be used as a trait object
        let transport: Box<dyn McpTransport> =
            Box::new(HttpTransport::new("test", config).unwrap());

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "test");
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_http_transport_set_notification_handler() {
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new("test", config).unwrap();

        // Should not panic when setting notification handler
        transport.set_notification_handler(Box::new(|_| {
            // This won't be called for HTTP transport
        }));

        // Transport should still work
        assert!(transport.is_alive().await);
    }

    #[test]
    fn parse_sse_picks_response_matching_id_among_interleaved_frames() {
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n",
            "\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n",
            "\n",
        );
        let resp = parse_sse_response(body, 7).expect("response present");
        assert_eq!(resp.id, Some(7));
        assert!(resp.result.is_some());
    }

    #[test]
    fn parse_sse_handles_crlf_and_multiline_data() {
        let body = "data: {\"jsonrpc\":\"2.0\",\r\ndata: \"id\":3,\"result\":{}}\r\n\r\n";
        let resp = parse_sse_response(body, 3).expect("response present");
        assert_eq!(resp.id, Some(3));
    }

    #[test]
    fn parse_sse_ignores_wrong_id_and_server_requests() {
        // A server-initiated request frame has an id but no result/error;
        // a response to a different id must also be skipped.
        let body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"sampling/createMessage\",\"params\":{}}\n",
            "\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}\n",
            "\n",
        );
        assert!(parse_sse_response(body, 9).is_none());
    }
}
