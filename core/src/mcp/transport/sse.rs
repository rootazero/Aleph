//! SSE (Server-Sent Events) Transport for Remote MCP Servers
//!
//! Implements MCP communication with bidirectional support:
//! - Requests: HTTP POST to server endpoint
//! - Server notifications: SSE event stream for real-time updates
//!
//! This transport is ideal for remote MCP servers that need to push
//! notifications to clients (e.g., tools/listChanged, resources/updated).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use reqwest_eventsource::{Event, EventSource};
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::{mpsc, RwLock};

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};

use super::sse_events::SseEvent;

/// Callback type for server-initiated requests (sampling, etc.)
pub type RequestCallback = Box<dyn Fn(u64, &str, Option<serde_json::Value>) + Send + Sync>;

/// SSE transport configuration
#[derive(Debug, Clone)]
pub struct SseTransportConfig {
    /// Server URL for POST requests (e.g., "https://example.com/mcp")
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
/// use aether_core::mcp::transport::{SseTransport, SseTransportConfig};
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
    /// HTTP client for POST requests
    client: Client,
    /// Connection state
    alive: Arc<RwLock<bool>>,
    /// Notification handler
    notification_handler: Arc<RwLock<Option<NotificationCallback>>>,
    /// Handler for server-initiated requests (sampling, etc.)
    request_handler: Arc<TokioMutex<Option<RequestCallback>>>,
    /// Shutdown signal sender
    shutdown_tx: RwLock<Option<mpsc::Sender<()>>>,
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
    pub fn new(name: impl Into<String>, config: SseTransportConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            server_name: name.into(),
            config,
            client,
            alive: Arc::new(RwLock::new(true)),
            notification_handler: Arc::new(RwLock::new(None)),
            request_handler: Arc::new(TokioMutex::new(None)),
            shutdown_tx: RwLock::new(None),
        }
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
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        {
            let mut tx = self.shutdown_tx.write().await;
            *tx = Some(shutdown_tx);
        }

        let sse_url = format!("{}/events", self.config.url.trim_end_matches('/'));
        let server_name = self.server_name.clone();
        let headers = self.config.headers.clone();
        let notification_handler = Arc::clone(&self.notification_handler);
        let request_handler = Arc::clone(&self.request_handler);
        let alive = Arc::clone(&self.alive);

        tokio::spawn(async move {
            tracing::info!(
                server = %server_name,
                url = %sse_url,
                "Starting SSE event listener"
            );

            // Create a client specifically for SSE (no timeout for long-lived connection)
            let sse_client = Client::builder()
                .build()
                .expect("Failed to create SSE client");

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::info!(server = %server_name, "SSE listener shutdown requested");
                        break;
                    }
                    result = Self::listen_for_events(&sse_client, &sse_url, &headers, &notification_handler, &request_handler, &server_name) => {
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
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }

            tracing::info!(server = %server_name, "SSE listener stopped");
        });

        Ok(())
    }

    /// Listen for SSE events from the server
    async fn listen_for_events(
        client: &Client,
        url: &str,
        headers: &HashMap<String, String>,
        notification_handler: &Arc<RwLock<Option<NotificationCallback>>>,
        request_handler: &Arc<TokioMutex<Option<RequestCallback>>>,
        server_name: &str,
    ) -> Result<()> {
        // Build request with headers
        let mut request = client.get(url);
        request = request.header("Accept", "text/event-stream");
        for (key, value) in headers {
            request = request.header(key, value);
        }

        let mut es = EventSource::new(request).map_err(|e| {
            AlephError::IoError(format!("Failed to create EventSource: {}", e))
        })?;

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
                        server_name,
                    )
                    .await;
                }
                Err(e) => {
                    // Check if it's a fatal error
                    let error_str = e.to_string();
                    if error_str.contains("connection") || error_str.contains("closed") {
                        tracing::warn!(server = %server_name, error = %e, "SSE connection error");
                        return Err(AlephError::IoError(format!("SSE connection error: {}", e)));
                    } else {
                        tracing::debug!(server = %server_name, error = %e, "SSE non-fatal error");
                    }
                }
            }
        }

        tracing::debug!(server = %server_name, "SSE stream ended");
        Ok(())
    }

    /// Handle a parsed SSE event
    async fn handle_sse_event(
        event: SseEvent,
        notification_handler: &Arc<RwLock<Option<NotificationCallback>>>,
        request_handler: &Arc<TokioMutex<Option<RequestCallback>>>,
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

                if let Some(ref handler) = *notification_handler.read().await {
                    handler(json_notif);
                }
            }
            SseEvent::Request(req) => {
                tracing::debug!(
                    server = %server_name,
                    method = %req.method,
                    id = req.id,
                    "Received SSE request (server-initiated RPC)"
                );

                // Handle server-initiated requests like sampling/createMessage
                if let Some(ref handler) = *request_handler.lock().await {
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
                tracing::info!(
                    server = %server_name,
                    endpoint = %url,
                    "Received endpoint URL from server"
                );
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

    /// Build request with configured headers
    fn build_request(&self, body: String) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.config.url)
            .header("Content-Type", "application/json");

        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }

        req.body(body)
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let body = serde_json::to_string(request).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize request: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending SSE/HTTP request"
        );

        let response = self.build_request(body).send().await.map_err(|e| {
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

        let text = response.text().await.map_err(|e| {
            AlephError::IoError(format!("Failed to read response: {}", e))
        })?;

        serde_json::from_str(&text).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to parse response from '{}': {} (body: {})",
                self.server_name, e, text
            ))
        })
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        let body = serde_json::to_string(notification).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize notification: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %notification.method,
            "Sending SSE/HTTP notification"
        );

        let response = self.build_request(body).send().await.map_err(|e| {
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

        // Store handler for SSE events
        // Use try_write to avoid blocking - if we can't get the lock immediately,
        // spawn a task to set it
        if let Ok(mut h) = self.notification_handler.try_write() {
            *h = Some(handler);
        } else {
            // If we can't get the lock, spawn a task to set it later
            let notification_handler = Arc::clone(&self.notification_handler);
            let server_name = self.server_name.clone();
            tokio::spawn(async move {
                let mut h = notification_handler.write().await;
                *h = Some(handler);
                tracing::debug!(
                    server = %server_name,
                    "SSE notification handler set via background task"
                );
            });
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SseTransport {
    /// Set handler for server-initiated requests (like sampling/createMessage)
    pub fn set_request_handler(&self, handler: RequestCallback) {
        tracing::debug!(
            server = %self.server_name,
            "Setting SSE request handler"
        );

        let request_handler = Arc::clone(&self.request_handler);
        tokio::spawn(async move {
            let mut h = request_handler.lock().await;
            *h = Some(handler);
        });
    }

    /// Send a response to a server-initiated request
    ///
    /// Used for responding to sampling/createMessage and other server-initiated RPCs.
    pub async fn send_response(&self, request_id: u64, result: serde_json::Value) -> Result<()> {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(request_id),
            result: Some(result),
            error: None,
        };

        self.send_json_rpc_response(&response).await?;

        tracing::debug!(
            server = %self.server_name,
            request_id = request_id,
            "Sent response to server-initiated request"
        );

        Ok(())
    }

    /// Send an error response to a server-initiated request
    pub async fn send_error_response(
        &self,
        request_id: u64,
        code: i32,
        message: &str,
    ) -> Result<()> {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(request_id),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        };

        self.send_json_rpc_response(&response).await?;

        tracing::debug!(
            server = %self.server_name,
            request_id = request_id,
            code = code,
            "Sent error response to server-initiated request"
        );

        Ok(())
    }

    /// Internal helper to send a JSON-RPC response via HTTP POST
    async fn send_json_rpc_response(&self, response: &JsonRpcResponse) -> Result<()> {
        let response_json = serde_json::to_string(response).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize response: {}", e))
        })?;

        let http_response = self
            .build_request(response_json)
            .send()
            .await
            .map_err(|e| {
                AlephError::IoError(format!("Failed to send response: {}", e))
            })?;

        if !http_response.status().is_success() {
            return Err(AlephError::IoError(format!(
                "Server returned error status: {}",
                http_response.status()
            )));
        }

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

        let transport = SseTransport::new("test-sse", config);
        assert_eq!(transport.server_name(), "test-sse");
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_close() {
        let config = SseTransportConfig::default();
        let transport = SseTransport::new("test", config);

        assert!(transport.is_alive().await);
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
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
        let transport: Box<dyn McpTransport> = Box::new(SseTransport::new("test", config));

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "test");
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_start_event_listener() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(300),
        };

        let transport = SseTransport::new("test-sse", config);

        // Starting the event listener should not fail
        transport.start_event_listener().await.unwrap();

        // Close should shutdown the listener gracefully
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_sse_transport_set_notification_handler() {
        let config = SseTransportConfig::default();
        let transport = SseTransport::new("test", config);

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
        let transport = SseTransport::new("test", config);

        // Should not panic when setting request handler
        transport.set_request_handler(Box::new(|id, method, _params| {
            tracing::info!(id = id, method = method, "Received request");
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
                "content": {"type": "text", "text": "Hello from Aether!"}
            })),
            error: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":123"));
        assert!(json.contains("\"result\""));
        assert!(json.contains("Hello from Aether!"));
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
}
