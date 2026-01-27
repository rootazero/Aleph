//! HTTP Transport for Remote MCP Servers
//!
//! Implements MCP communication over HTTP POST requests.
//! Suitable for stateless remote servers that support JSON-RPC over HTTP.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::RwLock;

use crate::error::{AetherError, Result};
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::mcp::transport::traits::{McpTransport, NotificationCallback};

/// HTTP transport configuration
#[derive(Debug, Clone)]
pub struct HttpTransportConfig {
    /// Server URL (e.g., "https://example.com/mcp")
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

/// HTTP transport for remote MCP servers
///
/// This transport implements the MCP protocol over HTTP POST requests.
/// Each JSON-RPC request is sent as a POST request to the configured URL,
/// and the response is expected to be a JSON-RPC response.
///
/// # Features
///
/// - Configurable URL, headers, and timeout
/// - Custom headers for authorization tokens
/// - Implements McpTransport trait for seamless integration
///
/// # Limitations
///
/// - HTTP transport does not support server-initiated notifications
/// - Each request is independent (no persistent connection)
///
/// # Example
///
/// ```ignore
/// use aether_core::mcp::transport::{HttpTransport, HttpTransportConfig};
///
/// let config = HttpTransportConfig {
///     url: "https://api.example.com/mcp".to_string(),
///     headers: [("Authorization".into(), "Bearer token".into())].into(),
///     timeout: Duration::from_secs(300),
/// };
///
/// let transport = HttpTransport::new("my-server", config);
/// ```
pub struct HttpTransport {
    /// Server name for logging
    server_name: String,
    /// Configuration
    config: HttpTransportConfig,
    /// HTTP client
    client: Client,
    /// Connection state
    alive: RwLock<bool>,
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
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be created (should not happen under normal circumstances)
    pub fn new(name: impl Into<String>, config: HttpTransportConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            server_name: name.into(),
            config,
            client,
            alive: RwLock::new(true),
            _notification_handler: RwLock::new(None),
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
impl McpTransport for HttpTransport {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let body = serde_json::to_string(request).map_err(|e| {
            AetherError::IoError(format!("Failed to serialize request: {}", e))
        })?;

        tracing::debug!(
            server = %self.server_name,
            method = %request.method,
            "Sending HTTP request"
        );

        let response = self.build_request(body).send().await.map_err(|e| {
            AetherError::IoError(format!(
                "HTTP request to '{}' failed: {}",
                self.server_name, e
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AetherError::IoError(format!(
                "HTTP {} from '{}': {}",
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
            "Sending HTTP notification"
        );

        let response = self.build_request(body).send().await.map_err(|e| {
            AetherError::IoError(format!(
                "HTTP notification to '{}' failed: {}",
                self.server_name, e
            ))
        })?;

        if !response.status().is_success() {
            tracing::warn!(
                server = %self.server_name,
                status = %response.status(),
                "HTTP notification returned non-success status"
            );
        }

        Ok(())
    }

    async fn is_alive(&self) -> bool {
        *self.alive.read().await
    }

    async fn close(&self) -> Result<()> {
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

        let transport = HttpTransport::new("test-server", config);
        assert_eq!(transport.server_name(), "test-server");
        assert!(transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_http_transport_close() {
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new("test", config);

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
        let transport: Box<dyn McpTransport> = Box::new(HttpTransport::new("test", config));

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "test");
        transport.close().await.unwrap();
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_http_transport_set_notification_handler() {
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new("test", config);

        // Should not panic when setting notification handler
        transport.set_notification_handler(Box::new(|_| {
            // This won't be called for HTTP transport
        }));

        // Transport should still work
        assert!(transport.is_alive().await);
    }
}
