//! MCP Transport Trait
//!
//! Defines the abstract interface for all MCP transport implementations.
//! This allows for different transport mechanisms (stdio, HTTP, SSE) while
//! maintaining a consistent API for the MCP client.

use async_trait::async_trait;

use crate::error::Result;
use crate::mcp::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Callback type for handling server-initiated notifications
///
/// MCP servers can send notifications at any time (e.g., tools/listChanged).
/// This callback allows transports to forward these notifications to interested parties.
pub type NotificationCallback = Box<dyn Fn(JsonRpcNotification) + Send + Sync>;

/// Abstract transport interface for MCP communication
///
/// This trait defines the common interface that all MCP transports must implement.
/// It supports both request/response patterns and fire-and-forget notifications.
///
/// # Implementations
///
/// - `StdioTransport` - Communicates with local MCP servers via subprocess stdio
/// - `HttpTransport` - Communicates with remote MCP servers via HTTP POST (planned)
/// - `SseTransport` - Communicates with remote MCP servers via HTTP + SSE (planned)
#[async_trait]
pub trait McpTransport: Send + Sync + std::any::Any {
    /// Send a JSON-RPC request and wait for the response
    ///
    /// This method sends a request to the MCP server and blocks until
    /// a response is received or an error occurs (including timeout).
    ///
    /// # Arguments
    /// * `request` - The JSON-RPC request to send
    ///
    /// # Returns
    /// * `Ok(JsonRpcResponse)` - The response from the server
    /// * `Err(AlephError)` - If sending failed, timeout occurred, or parsing failed
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Send a JSON-RPC notification (no response expected)
    ///
    /// Notifications are fire-and-forget messages. Per JSON-RPC 2.0 spec,
    /// the server MUST NOT reply to a notification.
    ///
    /// # Arguments
    /// * `notification` - The JSON-RPC notification to send
    ///
    /// # Returns
    /// * `Ok(())` - If the notification was sent successfully
    /// * `Err(AlephError)` - If sending failed
    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()>;

    /// Check if the transport connection is still alive
    ///
    /// For stdio transports, this checks if the process is running.
    /// For HTTP transports, this may perform a health check or return cached status.
    ///
    /// # Returns
    /// * `true` - The connection is alive and ready
    /// * `false` - The connection is dead or unhealthy
    async fn is_alive(&self) -> bool;

    /// Close the transport connection gracefully
    ///
    /// This should perform any cleanup necessary, such as:
    /// - Terminating subprocess for stdio transport
    /// - Closing HTTP connections for HTTP transport
    /// - Stopping SSE event stream for SSE transport
    ///
    /// # Returns
    /// * `Ok(())` - If closed successfully
    /// * `Err(AlephError)` - If an error occurred during close
    async fn close(&self) -> Result<()>;

    /// Get the server name for this transport
    ///
    /// This is primarily used for logging and debugging purposes.
    fn server_name(&self) -> &str;

    /// Set a handler for server-initiated notifications
    ///
    /// MCP servers can send notifications at any time to inform clients
    /// about state changes (e.g., tools/listChanged, resources/updated).
    ///
    /// The default implementation does nothing, as not all transports
    /// may support receiving notifications.
    ///
    /// # Arguments
    /// * `handler` - Callback function to be invoked when a notification is received
    fn set_notification_handler(&self, _handler: NotificationCallback) {
        // Default no-op implementation
        // Transports that support notifications should override this
    }

    /// Get a reference to the transport as Any for downcasting
    ///
    /// This enables type-specific operations on transports when needed,
    /// such as setting SSE-specific request handlers for sampling.
    fn as_any(&self) -> &dyn std::any::Any;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::sync_primitives::{AtomicBool, AtomicUsize, Ordering};
    use crate::sync_primitives::Arc;
    use tokio::sync::Mutex;

    /// Mock transport for testing the trait contract
    struct MockTransport {
        name: String,
        is_alive: AtomicBool,
        request_count: AtomicUsize,
        notification_count: AtomicUsize,
        /// Stores the last request for verification
        last_request: Mutex<Option<JsonRpcRequest>>,
        /// Predetermined response to return
        response: Mutex<Option<JsonRpcResponse>>,
        /// Whether to simulate a failure
        should_fail: AtomicBool,
    }

    impl MockTransport {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                is_alive: AtomicBool::new(true),
                request_count: AtomicUsize::new(0),
                notification_count: AtomicUsize::new(0),
                last_request: Mutex::new(None),
                response: Mutex::new(None),
                should_fail: AtomicBool::new(false),
            }
        }

        fn with_response(name: impl Into<String>, response: JsonRpcResponse) -> Self {
            Self {
                name: name.into(),
                is_alive: AtomicBool::new(true),
                request_count: AtomicUsize::new(0),
                notification_count: AtomicUsize::new(0),
                last_request: Mutex::new(None),
                response: Mutex::new(Some(response)),
                should_fail: AtomicBool::new(false),
            }
        }

        fn set_should_fail(&self, fail: bool) {
            self.should_fail.store(fail, Ordering::SeqCst);
        }

        fn set_alive(&self, alive: bool) {
            self.is_alive.store(alive, Ordering::SeqCst);
        }

        fn request_count(&self) -> usize {
            self.request_count.load(Ordering::SeqCst)
        }

        fn notification_count(&self) -> usize {
            self.notification_count.load(Ordering::SeqCst)
        }

        async fn last_request(&self) -> Option<JsonRpcRequest> {
            self.last_request.lock().await.clone()
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
            if self.should_fail.load(Ordering::SeqCst) {
                return Err(crate::error::AlephError::IoError(
                    "Mock transport failure".to_string(),
                ));
            }

            self.request_count.fetch_add(1, Ordering::SeqCst);
            *self.last_request.lock().await = Some(request.clone());

            // Return predetermined response or a default success response
            let response = self.response.lock().await.clone().unwrap_or_else(|| {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: Some(request.id),
                    result: Some(json!({"status": "ok"})),
                    error: None,
                }
            });

            Ok(response)
        }

        async fn send_notification(&self, _notification: &JsonRpcNotification) -> Result<()> {
            if self.should_fail.load(Ordering::SeqCst) {
                return Err(crate::error::AlephError::IoError(
                    "Mock transport failure".to_string(),
                ));
            }

            self.notification_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn is_alive(&self) -> bool {
            self.is_alive.load(Ordering::SeqCst)
        }

        async fn close(&self) -> Result<()> {
            self.is_alive.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn server_name(&self) -> &str {
            &self.name
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[tokio::test]
    async fn test_mock_transport_send_request() {
        let transport = MockTransport::new("test-server");
        let request = JsonRpcRequest::new(1, "test/method");

        let response = transport.send_request(&request).await.unwrap();

        assert!(response.is_success());
        assert_eq!(response.id, Some(1));
        assert_eq!(transport.request_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_transport_with_custom_response() {
        let custom_response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(42),
            result: Some(json!({"tools": ["tool1", "tool2"]})),
            error: None,
        };

        let transport = MockTransport::with_response("test-server", custom_response);
        let request = JsonRpcRequest::new(42, "tools/list");

        let response = transport.send_request(&request).await.unwrap();

        assert_eq!(response.result.unwrap()["tools"][0], "tool1");
    }

    #[tokio::test]
    async fn test_mock_transport_send_notification() {
        let transport = MockTransport::new("test-server");
        let notification = JsonRpcNotification::new("notifications/initialized");

        let result = transport.send_notification(&notification).await;

        assert!(result.is_ok());
        assert_eq!(transport.notification_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_transport_is_alive() {
        let transport = MockTransport::new("test-server");

        assert!(transport.is_alive().await);

        transport.set_alive(false);
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_mock_transport_close() {
        let transport = MockTransport::new("test-server");

        assert!(transport.is_alive().await);

        transport.close().await.unwrap();

        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_mock_transport_server_name() {
        let transport = MockTransport::new("my-mcp-server");

        assert_eq!(transport.server_name(), "my-mcp-server");
    }

    #[tokio::test]
    async fn test_mock_transport_request_failure() {
        let transport = MockTransport::new("test-server");
        transport.set_should_fail(true);

        let request = JsonRpcRequest::new(1, "test/method");
        let result = transport.send_request(&request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_transport_notification_failure() {
        let transport = MockTransport::new("test-server");
        transport.set_should_fail(true);

        let notification = JsonRpcNotification::new("test/notify");
        let result = transport.send_notification(&notification).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_transport_last_request() {
        let transport = MockTransport::new("test-server");
        let request = JsonRpcRequest::with_params(5, "tools/call", json!({"name": "my_tool"}));

        transport.send_request(&request).await.unwrap();

        let last = transport.last_request().await.unwrap();
        assert_eq!(last.id, 5);
        assert_eq!(last.method, "tools/call");
    }

    #[tokio::test]
    async fn test_trait_object_usage() {
        // Verify that the trait can be used as a trait object (dyn McpTransport)
        let transport: Arc<dyn McpTransport> = Arc::new(MockTransport::new("dyn-test"));

        assert!(transport.is_alive().await);
        assert_eq!(transport.server_name(), "dyn-test");

        let request = JsonRpcRequest::new(1, "test");
        let response = transport.send_request(&request).await.unwrap();
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_default_notification_handler() {
        // The default implementation should be a no-op
        let transport = MockTransport::new("test");

        // This should not panic
        transport.set_notification_handler(Box::new(|_| {
            // This won't be called because MockTransport doesn't override the default
        }));

        // Verify transport still works
        assert!(transport.is_alive().await);
    }
}
