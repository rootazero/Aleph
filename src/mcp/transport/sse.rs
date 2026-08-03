//! SSE (Server-Sent Events) Transport for Remote MCP Servers
//!
//! Implements MCP communication with bidirectional support:
//! - Requests: HTTP POST to server endpoint
//! - Server notifications: SSE event stream for real-time updates
//!
//! This transport is ideal for remote MCP servers that need to push
//! notifications to clients (e.g., tools/listChanged, resources/updated).

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use reqwest_eventsource::{Event, EventSource};
use tokio::sync::{mpsc, RwLock};

use crate::error::{AlephError, Result};
#[cfg(test)]
use crate::mcp::jsonrpc::JsonRpcError;
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};
use crate::security::ssrf::{validate_url_with_pinned, SsrfPolicy};

use super::sse_events::SseEvent;

/// Callback type for server-initiated requests (sampling, etc.)
pub type RequestCallback =
    Box<dyn Fn(serde_json::Value, &str, Option<serde_json::Value>) + Send + Sync>;

/// SSE transport configuration
#[derive(Debug, Clone)]
pub struct SseTransportConfig {
    /// Server URL for POST requests (e.g., "<https://example.com/mcp>")
    pub url: String,
    /// Custom HTTP headers (for auth tokens, etc.)
    pub headers: HashMap<String, String>,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for SseTransportConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        }
    }
}

/// SSE transport for remote MCP servers with server-initiated notifications
///
/// This transport combines HTTP POST requests (for client-to-server communication)
/// with Server-Sent Events (for server-to-client notifications).
///
/// # Architecture
///
/// ```text
/// Client                          Server
///   |                               |
///   |--- HTTP POST (request) ------>|
///   |<-- HTTP Response -------------|
///   |                               |
///   |<-- SSE Event (notification) --|
///   |<-- SSE Event (notification) --|
/// ```
///
/// # Example
///
/// ```ignore
/// use alephcore::mcp::transport::{SseTransport, SseTransportConfig};
///
/// let config = SseTransportConfig {
///     url: "https://api.example.com/mcp".to_string(),
///     headers: [("Authorization".into(), "Bearer token".into())].into(),
///     timeout: Duration::from_secs(300),
/// };
///
/// let transport = SseTransport::new("my-sse-server", config);
/// transport.start_event_listener().await?;
/// ```
pub struct SseTransport {
    /// Server name for logging
    server_name: String,
    /// Configuration
    config: SseTransportConfig,
    /// Connection state
    alive: Arc<RwLock<bool>>,
    /// Notification handler (`crate::sync_primitives::Mutex` wrapped in Arc so the spawned
    /// SSE listener task can access it without `block_in_place`)
    notification_handler: Arc<crate::sync_primitives::Mutex<Option<NotificationCallback>>>,
    /// Handler for server-initiated requests (sampling, etc.)
    request_handler: Arc<crate::sync_primitives::Mutex<Option<RequestCallback>>>,
    /// POST target announced by the server's `endpoint` event, once it arrives.
    /// `None` until then — see [`SseTransport::post_url`].
    post_endpoint: Arc<RwLock<Option<String>>>,
    /// Shutdown signal sender
    shutdown_tx: RwLock<Option<mpsc::Sender<()>>>,
    /// Handle for the spawned SSE listener task
    listener_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
}

impl SseTransport {
    /// Create a new SSE transport
    ///
    /// # Arguments
    ///
    /// * `name` - Server name for logging and identification
    /// * `config` - SSE transport configuration
    ///
    /// # Note
    ///
    /// After creating the transport, call `start_event_listener()` to begin
    /// receiving server-sent notifications.
    pub fn new(name: impl Into<String>, config: SseTransportConfig) -> Result<Self> {
        Ok(Self {
            server_name: name.into(),
            config,
            alive: Arc::new(RwLock::new(true)),
            notification_handler: Arc::new(crate::sync_primitives::Mutex::new(None)),
            request_handler: Arc::new(crate::sync_primitives::Mutex::new(None)),
            post_endpoint: Arc::new(RwLock::new(None)),
            shutdown_tx: RwLock::new(None),
            listener_handle: RwLock::new(None),
        })
    }

    /// Build a `reqwest::Client` whose resolver is pinned to the SSRF-validated
    /// address for the URL's host, closing the rebinding window between
    /// validation and reqwest's own resolver. IP-literal URLs and disabled
    /// policies skip the `.resolve()` rule.
    pub(crate) async fn build_pinned_client(
        url_str: &str,
        timeout: Option<Duration>,
    ) -> std::result::Result<(reqwest::Url, Client), AlephError> {
        let (url, pinned) = validate_url_with_pinned(url_str, &SsrfPolicy::default())
            .await
            .map_err(|e| AlephError::IoError(format!("SSRF blocked: {e}")))?;
        let host = url
            .host_str()
            .ok_or_else(|| AlephError::IoError("URL has no host".into()))?
            .to_string();
        let mut builder = Client::builder().redirect(reqwest::redirect::Policy::none());
        if let Some(addr) = pinned {
            builder = builder.resolve(&host, addr);
        }
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        let client = builder
            .build()
            .map_err(|e| AlephError::IoError(format!("Failed to build client: {e}")))?;
        Ok((url, client))
    }

    /// Resolve an `endpoint` event payload against the stream URL.
    ///
    /// Legacy SSE servers announce the POST target as a relative reference
    /// (`/messages?sessionId=…`), so it has to be joined onto the stream URL
    /// rather than parsed standalone.
    ///
    /// Cross-origin announcements are refused: every POST carries
    /// `config.headers` — the server's auth material — so honouring a foreign
    /// origin here would hand those credentials to a host the user never
    /// configured. A server that wants a different origin is not a server we
    /// can keep a secret for.
    #[must_use]
    pub(crate) fn resolve_endpoint(stream_url: &str, announced: &str) -> Option<String> {
        let base = reqwest::Url::parse(stream_url).ok()?;
        let resolved = base.join(announced).ok()?;
        if resolved.origin() == base.origin() {
            Some(resolved.into())
        } else {
            None
        }
    }

    /// Where a POST goes: the server-announced endpoint once it has arrived,
    /// otherwise the configured URL.
    ///
    /// The fallback is what keeps servers that never send an `endpoint` event
    /// working — the pre-spec shape this transport used to assume
    /// unconditionally.
    async fn post_url(&self) -> String {
        self.post_endpoint
            .read()
            .await
            .clone()
            .unwrap_or_else(|| self.config.url.clone())
    }

    /// Start the SSE event listener
    ///
    /// This spawns a background task that listens for server-sent events
    /// and dispatches them to the notification handler.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the listener started successfully
    /// * `Err(AlephError)` - If starting the listener failed
    pub async fn start_event_listener(&self) -> Result<()> {
        let (validated_url, sse_client) = Self::build_pinned_client(&self.config.url, None)
            .await
            .map_err(|e| {
            AlephError::IoError(format!("SSRF blocked for '{}': {}", self.server_name, e))
        })?;

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        {
            let mut tx = self.shutdown_tx.write().await;
            *tx = Some(shutdown_tx);
        }

        // Abort any previously running listener before starting a new one.
        {
            let mut handle = self.listener_handle.write().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        // The configured URL *is* the SSE stream endpoint. This used to append
        // an invented `/events` segment, which no MCP revision specifies — a
        // spec-compliant legacy server answers that path with 404 and the
        // handshake never starts.
        let sse_url = validated_url.as_str().to_string();
        let server_name = self.server_name.clone();
        let headers = self.config.headers.clone();
        let notification_handler = Arc::clone(&self.notification_handler);
        let request_handler = Arc::clone(&self.request_handler);
        let post_endpoint = Arc::clone(&self.post_endpoint);
        let alive = Arc::clone(&self.alive);

        let handle = tokio::spawn(async move {
            tracing::info!(
                server = %server_name,
                url = %sse_url,
                "Starting SSE event listener"
            );

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::info!(server = %server_name, "SSE listener shutdown requested");
                        break;
                    }
                    result = Self::listen_for_events(&sse_client, &sse_url, &headers, &notification_handler, &request_handler, &post_endpoint, &server_name) => {
                        match result {
                            Ok(()) => {
                                tracing::debug!(server = %server_name, "SSE stream ended normally");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    server = %server_name,
                                    error = %e,
                                    "SSE stream error, will retry"
                                );
                            }
                        }

                        // Check if we should still be alive
                        if !*alive.read().await {
                            break;
                        }

                        // Wait before reconnecting
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                            _ = shutdown_rx.recv() => break,
                        }
                    }
                }
            }

            tracing::info!(server = %server_name, "SSE listener stopped");
        });

        {
            let mut guard = self.listener_handle.write().await;
            *guard = Some(handle);
        }

        Ok(())
    }

    /// Listen for SSE events from the server
    async fn listen_for_events(
        client: &Client,
        url: &str,
        headers: &HashMap<String, String>,
        notification_handler: &Arc<crate::sync_primitives::Mutex<Option<NotificationCallback>>>,
        request_handler: &Arc<crate::sync_primitives::Mutex<Option<RequestCallback>>>,
        post_endpoint: &Arc<RwLock<Option<String>>>,
        server_name: &str,
    ) -> Result<()> {
        // Build request with headers
        let mut request = client.get(url);
        request = request.header("Accept", "text/event-stream");
        for (key, value) in headers {
            request = request.header(key, value);
        }

        let mut es = EventSource::new(request)
            .map_err(|e| AlephError::IoError(format!("Failed to create EventSource: {e}")))?;

        tracing::debug!(server = %server_name, "SSE EventSource created, waiting for events");

        while let Some(event) = es.next().await {
            match event {
                Ok(Event::Open) => {
                    tracing::debug!(server = %server_name, "SSE connection opened");
                }
                Ok(Event::Message(msg)) => {
                    let sse_event = SseEvent::parse(&msg.event, &msg.data);
                    Self::handle_sse_event(
                        sse_event,
                        notification_handler,
                        request_handler,
                        post_endpoint,
                        url,
                        server_name,
                    )
                    .await;
                }
                Err(e) => {
                    // All SSE errors are treated as connection-level failures.
                    // The reconnect loop in start_event_listener handles retries.
                    tracing::warn!(server = %server_name, error = %e, "SSE stream error");
                    return Err(AlephError::IoError(format!("SSE stream error: {e}")));
                }
            }
        }

        tracing::debug!(server = %server_name, "SSE stream ended");
        Ok(())
    }

    /// Handle a parsed SSE event
    async fn handle_sse_event(
        event: SseEvent,
        notification_handler: &Arc<crate::sync_primitives::Mutex<Option<NotificationCallback>>>,
        request_handler: &Arc<crate::sync_primitives::Mutex<Option<RequestCallback>>>,
        post_endpoint: &Arc<RwLock<Option<String>>>,
        stream_url: &str,
        server_name: &str,
    ) {
        match event {
            SseEvent::Notification(notif) => {
                tracing::debug!(
                    server = %server_name,
                    method = %notif.method,
                    "Received SSE notification"
                );

                // Create JsonRpcNotification and dispatch
                let json_notif = JsonRpcNotification {
                    jsonrpc: notif.jsonrpc,
                    method: notif.method,
                    params: notif.params,
                };

                let handler = notification_handler
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(ref handler) = *handler {
                    handler(json_notif);
                }
            }
            SseEvent::Request(req) => {
                tracing::debug!(
                    server = %server_name,
                    method = %req.method,
                    id = %req.id,
                    "Received SSE request (server-initiated RPC)"
                );

                // Handle server-initiated requests like sampling/createMessage
                let handler = request_handler.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(ref handler) = *handler {
                    handler(req.id, &req.method, req.params);
                } else {
                    tracing::warn!(
                        server = %server_name,
                        method = %req.method,
                        "No handler registered for server-initiated requests"
                    );
                }
            }
            SseEvent::Endpoint { url } => {
                match Self::resolve_endpoint(stream_url, &url) {
                    Some(resolved) => {
                        tracing::info!(
                            server = %server_name,
                            endpoint = %resolved,
                            "Adopted server-announced POST endpoint"
                        );
                        *post_endpoint.write().await = Some(resolved);
                    }
                    None => {
                        // Keep POSTing to the configured URL rather than to a
                        // host the user never named.
                        tracing::warn!(
                            server = %server_name,
                            announced = %url,
                            stream = %stream_url,
                            "Ignoring endpoint event: unresolvable or cross-origin"
                        );
                    }
                }
            }
            SseEvent::Ping => {
                tracing::trace!(server = %server_name, "Received SSE ping");
            }
            SseEvent::Unknown { event_type, data } => {
                tracing::debug!(
                    server = %server_name,
                    event_type = %event_type,
                    data_len = data.len(),
                    "Received unknown SSE event"
                );
            }
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let (validated_url, client) =
            Self::build_pinned_client(&self.post_url().await, Some(self.config.timeout))
                .await
                .map_err(|e| {
                    AlephError::IoError(format!("SSRF blocked for '{}': {}", self.server_name, e))
                })?;

        let body = serde_json::to_string(request)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize request: {e}")))?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending SSE/HTTP request"
        );

        let mut req = client
            .post(validated_url.as_str())
            .header("Content-Type", "application/json");
        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }
        let response = req.body(body).send().await.map_err(|e| {
            AlephError::IoError(format!(
                "SSE request to '{}' failed: {}",
                self.server_name, e
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "SSE HTTP {} from '{}': {}",
                status, self.server_name, body
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to read response: {e}")))?;

        serde_json::from_str(&text).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to parse response from '{}': {} (body: {})",
                self.server_name, e, text
            ))
        })
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        let (validated_url, client) =
            Self::build_pinned_client(&self.post_url().await, Some(self.config.timeout))
                .await
                .map_err(|e| {
                    AlephError::IoError(format!("SSRF blocked for '{}': {}", self.server_name, e))
                })?;

        let body = serde_json::to_string(notification)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize notification: {e}")))?;

        tracing::debug!(
            server = %self.server_name,
            method = %notification.method,
            "Sending SSE/HTTP notification"
        );

        let mut req = client
            .post(validated_url.as_str())
            .header("Content-Type", "application/json");
        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }
        let response = req.body(body).send().await.map_err(|e| {
            AlephError::IoError(format!(
                "SSE notification to '{}' failed: {}",
                self.server_name, e
            ))
        })?;

        if !response.status().is_success() {
            tracing::warn!(
                server = %self.server_name,
                status = %response.status(),
                "SSE notification returned non-success status"
            );
        }

        Ok(())
    }

    async fn is_alive(&self) -> bool {
        *self.alive.read().await
    }

    async fn close(&self) -> Result<()> {
        // Send shutdown signal to SSE listener
        if let Some(tx) = self.shutdown_tx.read().await.as_ref() {
            let _ = tx.send(()).await;
        }

        let mut alive = self.alive.write().await;
        *alive = false;
        Ok(())
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }

    fn set_notification_handler(&self, handler: NotificationCallback) {
        tracing::debug!(
            server = %self.server_name,
            "Setting SSE notification handler"
        );

        let mut h = self
            .notification_handler
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *h = Some(handler);
    }

    async fn send_sampling_response(
        &self,
        request_id: serde_json::Value,
        result: serde_json::Value,
    ) -> Result<()> {
        self.send_response(request_id, result).await
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SseTransport {
    /// Set handler for server-initiated requests (like sampling/createMessage)
    ///
    /// Installs the handler synchronously to guarantee it is set before
    /// `start_event_listener()` begins dispatching incoming requests.
    pub fn set_request_handler(&self, handler: RequestCallback) {
        tracing::debug!(
            server = %self.server_name,
            "Setting SSE request handler"
        );

        let mut h = self
            .request_handler
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *h = Some(handler);
    }

    /// Send a response to a server-initiated request
    ///
    /// Used for responding to sampling/createMessage and other server-initiated RPCs.
    pub async fn send_response(
        &self,
        request_id: serde_json::Value,
        result: serde_json::Value,
    ) -> Result<()> {
        let (validated_url, client) =
            Self::build_pinned_client(&self.post_url().await, Some(self.config.timeout))
                .await
                .map_err(|e| {
                    AlephError::IoError(format!("SSRF blocked for '{}': {}", self.server_name, e))
                })?;

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": result,
        });

        let response_json = serde_json::to_string(&response)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize response: {e}")))?;

        let mut req = client
            .post(validated_url.as_str())
            .header("Content-Type", "application/json");
        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }
        let http_response = req
            .body(response_json)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to send response: {e}")))?;

        if !http_response.status().is_success() {
            return Err(AlephError::IoError(format!(
                "Server returned error status: {}",
                http_response.status()
            )));
        }

        tracing::debug!(
            server = %self.server_name,
            request_id = %request_id,
            "Sent response to server-initiated request"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_transport_config() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp/sse".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        assert_eq!(config.url, "https://example.com/mcp/sse");
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_sse_transport_config_default() {
        let config = SseTransportConfig::default();

        assert!(config.url.is_empty());
        assert!(config.headers.is_empty());
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_sse_transport_creation() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        let transport = SseTransport::new("test-sse", config).unwrap();
        assert_eq!(transport.server_name(), "test-sse");
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_close() {
        let config = SseTransportConfig::default();
        let transport = SseTransport::new("test", config).unwrap();

        assert!(transport.is_alive().await);
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    /// The spec's own shape: a relative reference joined onto the stream URL,
    /// query string and all (session id lives there).
    #[test]
    fn endpoint_event_resolves_relative_against_stream_url() {
        assert_eq!(
            SseTransport::resolve_endpoint(
                "https://api.example.com/mcp/sse",
                "/messages?sessionId=abc"
            ),
            Some("https://api.example.com/messages?sessionId=abc".to_string())
        );
        assert_eq!(
            SseTransport::resolve_endpoint("https://api.example.com/mcp/sse", "messages"),
            Some("https://api.example.com/mcp/messages".to_string())
        );
    }

    /// Every POST carries the configured auth headers, so an endpoint pointing
    /// at another origin is refused rather than followed.
    #[test]
    fn endpoint_event_refuses_cross_origin() {
        for announced in [
            "https://evil.example.net/messages",
            "http://api.example.com/messages", // scheme differs
            "https://api.example.com:8443/messages", // port differs
        ] {
            assert_eq!(
                SseTransport::resolve_endpoint("https://api.example.com/mcp/sse", announced),
                None,
                "should refuse {announced}"
            );
        }
    }

    /// Until the server announces one, POSTs keep going to the configured URL —
    /// servers that never send `endpoint` must keep working.
    #[tokio::test]
    async fn post_url_falls_back_to_configured_url() {
        let config = SseTransportConfig {
            url: "https://api.example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };
        let transport = SseTransport::new("test", config).unwrap();
        assert_eq!(transport.post_url().await, "https://api.example.com/mcp");

        *transport.post_endpoint.write().await =
            Some("https://api.example.com/messages?sessionId=abc".to_string());
        assert_eq!(
            transport.post_url().await,
            "https://api.example.com/messages?sessionId=abc"
        );
    }

    #[test]
    fn test_sse_transport_config_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        headers.insert("X-Session-Id".to_string(), "session-abc".to_string());

        let config = SseTransportConfig {
            url: "https://api.example.com/mcp".to_string(),
            headers,
            timeout: Duration::from_secs(60),
        };

        assert!(config.headers.contains_key("Authorization"));
        assert!(config.headers.contains_key("X-Session-Id"));
        assert_eq!(config.timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_sse_transport_implements_mcp_transport_trait() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        // Verify it can be used as a trait object
        let transport: Box<dyn McpTransport> = Box::new(SseTransport::new("test", config).unwrap());

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "test");
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_start_event_listener() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);

        let config = SseTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        let transport = SseTransport::new("test-sse", config).unwrap();

        // Starting the event listener should not fail
        transport.start_event_listener().await.unwrap();

        // Close should shutdown the listener gracefully
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_set_notification_handler() {
        let config = SseTransportConfig::default();
        let transport = SseTransport::new("test", config).unwrap();

        // Should not panic when setting notification handler
        transport.set_notification_handler(Box::new(|notification| {
            tracing::info!(method = %notification.method, "Received notification");
        }));

        // Transport should still work
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_set_request_handler() {
        let config = SseTransportConfig::default();
        let transport = SseTransport::new("test", config).unwrap();

        // Should not panic when setting request handler
        transport.set_request_handler(Box::new(|id, method, _params| {
            tracing::info!(id = %id, method = method, "Received request");
        }));

        // Transport should still work
        assert!(transport.is_alive().await);
    }

    #[test]
    fn test_json_rpc_response_construction_success() {
        // Test that we can construct a success response
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(42),
            result: Some(serde_json::json!({"text": "Hello"})),
            error: None,
        };

        assert!(response.is_success());
        assert!(!response.is_error());
        assert_eq!(response.id, Some(42));
    }

    #[test]
    fn test_json_rpc_response_construction_error() {
        // Test that we can construct an error response
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(42),
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid request".to_string(),
                data: None,
            }),
        };

        assert!(!response.is_success());
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, -32600);
    }

    #[test]
    fn test_json_rpc_response_serialization() {
        // Test that response serializes correctly for server-initiated request responses
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(123),
            result: Some(serde_json::json!({
                "role": "assistant",
                "content": {"type": "text", "text": "Hello from Aleph!"}
            })),
            error: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":123"));
        assert!(json.contains("\"result\""));
        assert!(json.contains("Hello from Aleph!"));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_json_rpc_error_response_serialization() {
        // Test that error response serializes correctly
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(456),
            result: None,
            error: Some(JsonRpcError {
                code: -32001,
                message: "Sampling not supported".to_string(),
                data: None,
            }),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":456"));
        assert!(json.contains("\"error\""));
        assert!(json.contains("-32001"));
        assert!(json.contains("Sampling not supported"));
        assert!(!json.contains("\"result\""));
    }

    #[tokio::test]
    async fn build_pinned_client_succeeds_for_hostname_with_public_ip() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["8.8.8.8".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let result = SseTransport::build_pinned_client(
            "https://example.com/mcp",
            Some(Duration::from_secs(30)),
        )
        .await;
        let (url, _client) = result.expect("public hostname must build a pinned client");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.scheme(), "https");
    }

    #[tokio::test]
    async fn build_pinned_client_rejects_hostname_resolving_to_loopback() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "evil.example".to_string(),
            vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let result = SseTransport::build_pinned_client("http://evil.example/mcp", None).await;
        assert!(
            matches!(result, Err(AlephError::IoError(ref msg)) if msg.starts_with("SSRF blocked")),
            "hostname → 127.0.0.1 must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn build_pinned_client_succeeds_for_ip_literal_without_pin() {
        let result = SseTransport::build_pinned_client("https://8.8.8.8/mcp", None).await;
        let (url, _client) = result.expect("public IP literal must build a client");
        assert_eq!(url.host_str(), Some("8.8.8.8"));
    }

    #[tokio::test]
    async fn build_pinned_client_rejects_benchmark_range_resolution() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "bench.example".to_string(),
            vec!["198.18.0.5".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let result = SseTransport::build_pinned_client("http://bench.example/mcp", None).await;
        assert!(
            matches!(result, Err(AlephError::IoError(ref msg)) if msg.starts_with("SSRF blocked")),
            "198.18.0.0/15 resolution must be blocked — got {result:?}"
        );
    }
}
