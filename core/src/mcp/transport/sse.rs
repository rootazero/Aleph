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
use reqwest::Client;
use tokio::sync::{mpsc, RwLock};

use crate::error::{AetherError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};

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
            timeout: Duration::from_secs(30),
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
///     timeout: Duration::from_secs(30),
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
    /// * `Err(AetherError)` - If starting the listener failed
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
                    result = Self::listen_for_events(&sse_client, &sse_url, &headers, &notification_handler) => {
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

    /// Listen for SSE events
    ///
    /// This is a placeholder implementation. In production, you would use
    /// a proper SSE client library like reqwest-eventsource or eventsource-client.
    async fn listen_for_events(
        _client: &Client,
        _url: &str,
        _headers: &HashMap<String, String>,
        _notification_handler: &Arc<RwLock<Option<NotificationCallback>>>,
    ) -> Result<()> {
        // Placeholder: In a real implementation, this would:
        // 1. Connect to the SSE endpoint
        // 2. Parse incoming events
        // 3. Deserialize notifications
        // 4. Call the notification handler
        //
        // For now, we just sleep to simulate a long-lived connection
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(())
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
            AetherError::IoError(format!("Failed to serialize request: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending SSE/HTTP request"
        );

        let response = self.build_request(body).send().await.map_err(|e| {
            AetherError::IoError(format!(
                "SSE request to '{}' failed: {}",
                self.server_name, e
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AetherError::IoError(format!(
                "SSE HTTP {} from '{}': {}",
                status, self.server_name, body
            )));
        }

        let text = response.text().await.map_err(|e| {
            AetherError::IoError(format!("Failed to read response: {}", e))
        })?;

        serde_json::from_str(&text).map_err(|e| {
            AetherError::IoError(format!(
                "Failed to parse response from '{}': {} (body: {})",
                self.server_name, e, text
            ))
        })
    }

    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()> {
        let body = serde_json::to_string(notification).map_err(|e| {
            AetherError::IoError(format!("Failed to serialize notification: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %notification.method,
            "Sending SSE/HTTP notification"
        );

        let response = self.build_request(body).send().await.map_err(|e| {
            AetherError::IoError(format!(
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_transport_config() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp/sse".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
        };

        assert_eq!(config.url, "https://example.com/mcp/sse");
        assert_eq!(config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_sse_transport_config_default() {
        let config = SseTransportConfig::default();

        assert!(config.url.is_empty());
        assert!(config.headers.is_empty());
        assert_eq!(config.timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_sse_transport_creation() {
        let config = SseTransportConfig {
            url: "https://example.com/mcp".to_string(),
            headers: HashMap::new(),
            timeout: Duration::from_secs(30),
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
            timeout: Duration::from_secs(30),
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
            timeout: Duration::from_secs(30),
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
}
