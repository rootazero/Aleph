//! Streamable HTTP Transport for Remote MCP Servers
//!
//! Implements MCP communication over HTTP POST for **both** shapes of the
//! Streamable HTTP transport, because which one applies is a property of the
//! server, not of Aleph:
//!
//! - **Modern (`2026-07-28`)** — stateless. No sessions, so no `Mcp-Session-Id`
//!   and no terminating `DELETE`; instead every POST mirrors body fields into
//!   the required `Mcp-Method` / `Mcp-Name` headers (plus any `Mcp-Param-*` the
//!   caller derived from the tool schema). Servers reject a request whose
//!   headers disagree with its body, so those values are derived here, from the
//!   very body being sent.
//! - **Legacy (`2025-03-26` … `2025-11-25`)** — session-bearing. The
//!   `Mcp-Session-Id` a server assigns on `initialize` is captured and echoed
//!   on every later message, `404` on a session-bearing request means the
//!   server expired it, and `close()` sends a best-effort `DELETE`.
//!
//! Common to both:
//!
//! - Every request advertises `Accept: application/json, text/event-stream`.
//!   The spec requires the client to list both; official SDK servers reject
//!   requests without it.
//! - POST responses delivered as `text/event-stream` are scanned for the
//!   JSON-RPC response matching the request id (servers may interleave
//!   request-scoped notifications on the same stream).
//! - A 4xx/5xx whose body is a JSON-RPC error response is surfaced as that
//!   error rather than as a transport failure. This is what lets the connection
//!   layer tell the eras apart: a modern server answers a request it dislikes
//!   with `400` plus a spec-reserved error code, whereas a legacy server
//!   confronted with a handshake-less request produces something else entirely.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::modern::headers as modern_headers;
use crate::mcp::modern::{McpDialect, MCP_MODERN_PROTOCOL_VERSION};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SafeFetchResponse, SsrfPolicy};

/// Header carrying the Streamable HTTP session identifier.
const SESSION_HEADER: &str = "Mcp-Session-Id";

/// Largest response body the HTTP transport will buffer for JSON parsing.
///
/// A hostile MCP server that returns a 2 GB body with `Content-Type:
/// application/json` would otherwise be fully read into memory before the
/// transport saw a single byte. The cap is a defence-in-depth bound; the
/// upstream `reqwest`/`tokio` read also still has the per-request timeout.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Largest single `data:` line `parse_sse_response` will buffer.
///
/// The SSE walker builds a `String` per event from the wire's `data: …`
/// lines. A server that emits a single unbounded line (rather than many
/// short ones) can otherwise OOM the daemon before the JSON parser sees a
/// byte. Picked well above the largest reasonable Streamable HTTP response.
const MAX_SSE_DATA_LINE_BYTES: usize = 1024 * 1024;

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
/// - No standalone server-to-client stream is opened, so unsolicited server
///   notifications (`listChanged`) are not received here. On a legacy server
///   that means the SSE transport is the one that hears them; on a modern
///   server the standalone stream no longer exists at all, and freshness comes
///   from the `ttlMs` hints on list results instead (see
///   [`crate::mcp::modern::cache`]).
/// - Server-requested sampling is **not** a limitation of this transport any
///   more: `2026-07-28` replaced server-initiated requests with Multi
///   Round-Trip Requests, which are ordinary retries and work here.
pub struct HttpTransport {
    /// Server name for logging
    server_name: String,
    /// Configuration
    config: HttpTransportConfig,
    /// Connection state
    alive: RwLock<bool>,
    /// Streamable HTTP session id assigned by the server on `initialize`.
    /// Only ever populated on the legacy path; revision 2026-07-28 removed
    /// protocol-level sessions.
    session_id: RwLock<Option<String>>,
    /// The dialect settled on for this connection — the era plus the revision
    /// to echo on `MCP-Protocol-Version`. `None` until the connection layer has
    /// probed, which is why the pre-probe default below has to be the modern
    /// one: the probe request itself must be a modern request for the server's
    /// answer to mean anything.
    ///
    /// A `std` lock (not tokio's) because it is written from the sync
    /// `set_dialect` trait method and only held to clone a small value.
    dialect: std::sync::RwLock<Option<McpDialect>>,
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
            dialect: std::sync::RwLock::new(None),
        })
    }

    /// The dialect in force, or the pre-probe default.
    ///
    /// Before the connection layer probes, the transport must already behave as
    /// a modern client: the probe is a modern request, and a legacy server's
    /// rejection of it is the very signal that selects the legacy path.
    fn dialect(&self) -> McpDialect {
        self.dialect
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or(McpDialect::Modern {
                version: MCP_MODERN_PROTOCOL_VERSION.to_string(),
            })
    }

    /// Build the header set for one HTTP message.
    ///
    /// `request` is `Some` only for JSON-RPC *requests*; `close()`'s session
    /// `DELETE` and notification POSTs pass `None`, the latter because this
    /// revision leaves header requirements for notification bodies undefined.
    ///
    /// Ordering is deliberate: operator-configured headers go in **first** so
    /// the protocol-owned ones overwrite rather than get overwritten. A
    /// configured `Mcp-Method` winning would make every request fail server
    /// validation with `HeaderMismatch`, and the operator would have no way to
    /// see why.
    fn request_headers(
        &self,
        dialect: &McpDialect,
        session: Option<&str>,
        request: Option<&JsonRpcRequest>,
        extra_headers: &[(String, String)],
    ) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        for (key, value) in &self.config.headers {
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|e| AlephError::IoError(format!("Invalid MCP header name: {e}")))?;
            let value = HeaderValue::from_str(value)
                .map_err(|e| AlephError::IoError(format!("Invalid MCP header value: {e}")))?;
            headers.insert(name, value);
        }

        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static(modern_headers::HEADER_PROTOCOL_VERSION),
            HeaderValue::from_str(dialect.version())
                .map_err(|e| AlephError::IoError(format!("Invalid MCP protocol version: {e}")))?,
        );

        if dialect.is_modern() {
            // Required on every modern POST, and derived from the body being
            // sent so the two cannot disagree.
            if let Some(request) = request {
                headers.insert(
                    HeaderName::from_static(modern_headers::HEADER_METHOD),
                    HeaderValue::from_str(&request.method).map_err(|e| {
                        AlephError::IoError(format!("Invalid MCP method for header: {e}"))
                    })?,
                );
                if let Some(name) =
                    modern_headers::name_header_value(&request.method, request.params.as_ref())
                {
                    headers.insert(
                        HeaderName::from_static(modern_headers::HEADER_NAME),
                        HeaderValue::from_str(&name).map_err(|e| {
                            AlephError::IoError(format!("Invalid MCP name for header: {e}"))
                        })?,
                    );
                }
            }
            for (name, value) in extra_headers {
                let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                    AlephError::IoError(format!("Invalid MCP parameter header name: {e}"))
                })?;
                let value = HeaderValue::from_str(value).map_err(|e| {
                    AlephError::IoError(format!("Invalid MCP parameter header value: {e}"))
                })?;
                headers.insert(name, value);
            }
        } else if let Some(session) = session {
            // Sessions exist only in the legacy shape.
            headers.insert(
                HeaderName::from_static("mcp-session-id"),
                HeaderValue::from_str(session)
                    .map_err(|e| AlephError::IoError(format!("Invalid MCP session id: {e}")))?,
            );
        }

        Ok(headers)
    }

    async fn send_body(
        &self,
        body: Vec<u8>,
        dialect: &McpDialect,
        session: Option<&str>,
        request: Option<&JsonRpcRequest>,
        extra_headers: &[(String, String)],
    ) -> Result<SafeFetchResponse> {
        let headers = self.request_headers(dialect, session, request, extra_headers)?;
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
///
/// A single `data:` line is capped at [`MAX_SSE_DATA_LINE_BYTES`]; a server
/// that emits a longer line is treated as a stream that will never yield the
/// expected response and the parse returns `None`. The wire-level
/// `MAX_RESPONSE_BYTES` cap on the body is the outer bound; this is the
/// inner one that prevents a single hostile event from allocating without
/// bound.
fn parse_sse_response(body: &str, expected_id: u64) -> Option<JsonRpcResponse> {
    let mut data = String::new();
    let mut found: Option<JsonRpcResponse> = None;
    let mut overflow: bool = false;

    fn flush(
        data: &mut String,
        found: &mut Option<JsonRpcResponse>,
        expected_id: u64,
        overflow: &mut bool,
    ) {
        if *overflow {
            // Don't bother parsing — we already know the event is unusable.
            data.clear();
            return;
        }
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
            flush(&mut data, &mut found, expected_id, &mut overflow);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !overflow {
                let piece = rest.strip_prefix(' ').unwrap_or(rest);
                if data.len() + piece.len() > MAX_SSE_DATA_LINE_BYTES {
                    overflow = true;
                    data.clear();
                } else {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(piece);
                }
            }
        }
        // Other SSE fields (event:, id:, retry:, comments) carry no payload.
    }
    flush(&mut data, &mut found, expected_id, &mut overflow);
    found
}

/// Read a JSON-RPC *error response* out of a non-2xx body.
///
/// A 4xx/5xx that carries one is the server answering the protocol, not the
/// transport failing — and on `400` it is the sole signal that separates a
/// modern server (spec-reserved code) from a legacy one (anything else). The
/// error may legitimately carry `"id": null`, so the id is not matched here;
/// the caller is not multiplexing on this path.
///
/// The HTTP status is folded into the message text because
/// [`crate::mcp::classify_mcp_error`] reads status codes (`401`, `503`, …) out
/// of the rendered string to pick a recovery hint. Surfacing the JSON-RPC error
/// instead of a transport error would otherwise silently drop that signal. The
/// numeric JSON-RPC `code` is left untouched — it is what the era probe reads.
fn parse_error_body(text: &str, status: reqwest::StatusCode) -> Option<JsonRpcResponse> {
    let mut response: JsonRpcResponse = serde_json::from_str(text).ok()?;
    let error = response.error.as_mut()?;
    if !error.message.contains(status.as_str()) {
        error.message = format!("{} (HTTP {})", error.message, status.as_u16());
    }
    Some(response)
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.send_request_with_headers(request, &[]).await
    }

    async fn send_request_with_headers(
        &self,
        request: &JsonRpcRequest,
        extra_headers: &[(String, String)],
    ) -> Result<JsonRpcResponse> {
        let body = serde_json::to_vec(request)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize request: {e}")))?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending HTTP request"
        );

        let dialect = self.dialect();
        let session = if dialect.is_modern() {
            None
        } else {
            self.session_id.read().await.clone()
        };
        let response = self
            .send_body(
                body,
                &dialect,
                session.as_deref(),
                Some(request),
                extra_headers,
            )
            .await?;
        let status = response.status;

        if !dialect.is_modern()
            && status == reqwest::StatusCode::NOT_FOUND
            && self.clear_expired_session().await
        {
            return Err(AlephError::IoError(format!(
                "MCP session for '{}' expired (HTTP 404); server requires re-initialization",
                self.server_name
            )));
        }

        if !status.is_success() {
            let body = String::from_utf8_lossy(&response.body);
            if let Some(error_response) = parse_error_body(&body, status) {
                return Ok(error_response);
            }
            return Err(AlephError::IoError(format!(
                "HTTP {} from '{}': {}",
                status, self.server_name, body
            )));
        }

        if !dialect.is_modern() {
            self.capture_session(&response.headers).await;
        }

        let is_sse = response
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));

        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err(AlephError::IoError(format!(
                "HTTP response from '{}' exceeds {} MB cap (was {} bytes); refusing to buffer",
                self.server_name,
                MAX_RESPONSE_BYTES / (1024 * 1024),
                response.body.len()
            )));
        }

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

        let dialect = self.dialect();
        let session = if dialect.is_modern() {
            None
        } else {
            self.session_id.read().await.clone()
        };
        let response = self
            .send_body(body, &dialect, session.as_deref(), None, &[])
            .await?;
        let status = response.status;

        if status.is_success() {
            if !dialect.is_modern() {
                self.capture_session(&response.headers).await;
            }
        } else {
            if !dialect.is_modern() && status == reqwest::StatusCode::NOT_FOUND {
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
        // Only ever populated on the legacy path, so a modern connection skips
        // the terminating DELETE without needing to ask which era it is.
        let session = self.session_id.write().await.take();
        if let Some(session) = session {
            let dialect = self.dialect();
            match self.request_headers(&dialect, Some(&session), None, &[]) {
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
        // in basic mode; the trait requires the hook, so accept and drop.
        tracing::debug!(
            server = %self.server_name,
            "Notification handler set (HTTP transport has limited notification support)"
        );
        // Could implement polling here in the future
        let _ = handler; // Acknowledge but don't use
    }

    fn mirrors_param_headers(&self) -> bool {
        true
    }

    fn set_dialect(&self, dialect: &McpDialect) {
        let mut slot = self.dialect.write().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref() != Some(dialect) {
            tracing::debug!(
                server = %self.server_name,
                version = dialect.version(),
                modern = dialect.is_modern(),
                "Settled MCP protocol dialect"
            );
            *slot = Some(dialect.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jsonrpc_error_body_is_surfaced_as_a_protocol_answer() {
        // The era probe reads the JSON-RPC code out of a 400; swallowing it as
        // a transport failure would leave nothing to tell the eras apart.
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32022,
            "message":"Unsupported protocol version",
            "data":{"supported":["2026-07-28"]}}}"#;

        let parsed = parse_error_body(body, reqwest::StatusCode::BAD_REQUEST).unwrap();
        let error = parsed.error.unwrap();

        assert_eq!(error.code, -32022);
        assert!(error.data.is_some());
    }

    #[test]
    fn the_http_status_survives_into_the_message() {
        // `classify_mcp_error` reads status codes out of the rendered string to
        // pick a recovery hint; dropping the status would silently downgrade a
        // 401 from "re-authenticate" to "unknown".
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"nope"}}"#;

        let unauthorized = parse_error_body(body, reqwest::StatusCode::UNAUTHORIZED).unwrap();
        let message = unauthorized.error.unwrap().message;
        assert!(message.contains("401"), "{message}");
        assert_eq!(
            crate::mcp::classify_mcp_error(&message),
            crate::mcp::McpErrorKind::AuthExpired
        );

        let unavailable = parse_error_body(body, reqwest::StatusCode::SERVICE_UNAVAILABLE).unwrap();
        let message = unavailable.error.unwrap().message;
        assert_eq!(
            crate::mcp::classify_mcp_error(&message),
            crate::mcp::McpErrorKind::Transient
        );
    }

    #[test]
    fn a_status_already_named_in_the_message_is_not_repeated() {
        let body =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"got 503 upstream"}}"#;

        let parsed = parse_error_body(body, reqwest::StatusCode::SERVICE_UNAVAILABLE).unwrap();
        let message = parsed.error.unwrap().message;

        assert_eq!(message, "got 503 upstream");
    }

    #[test]
    fn a_non_jsonrpc_error_page_stays_a_transport_failure() {
        // An HTML error page or an empty body is not the server answering the
        // protocol, and on the HTTP fallback path it is what identifies a
        // legacy server.
        assert!(parse_error_body(
            "<html>gateway timeout</html>",
            reqwest::StatusCode::BAD_GATEWAY
        )
        .is_none());
        assert!(parse_error_body("", reqwest::StatusCode::BAD_REQUEST).is_none());
        // A well-formed *success* response on a 4xx is not an error body either.
        assert!(parse_error_body(
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            reqwest::StatusCode::BAD_REQUEST
        )
        .is_none());
    }

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

    #[test]
    fn parse_sse_caps_a_single_data_line() {
        // A single data: line longer than the per-line cap is treated as a
        // stream that will never yield the expected response. The line
        // overflows the bound; the rest of the stream is ignored.
        let huge = "x".repeat(MAX_SSE_DATA_LINE_BYTES + 1024);
        let body = format!("data: {huge}\n\n");
        assert!(parse_sse_response(&body, 1).is_none());
    }
}
