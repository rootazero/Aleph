//! MCP Sampling Handler
//!
//! Handles server-initiated sampling/createMessage requests,
//! allowing MCP servers to call the host's LLM.

use crate::sync_primitives::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::client::McpClient;
use crate::mcp::context_injector::ContextInjector;
use crate::mcp::jsonrpc::mcp::{SamplingRequest, SamplingResponse};
#[cfg(test)]
use crate::mcp::jsonrpc::mcp::{PromptRole, SamplingContent, StopReason};

/// Callback for handling sampling requests
///
/// Takes a `SamplingRequest` and returns a Future that resolves to a `SamplingResponse`.
pub type SamplingCallback = Box<
    dyn Fn(
            SamplingRequest,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SamplingResponse>> + Send>>
        + Send
        + Sync,
>;

/// Manages sampling requests from MCP servers
pub struct SamplingHandler {
    /// Callback to invoke for sampling requests (Arc-wrapped for cheap cloning out of lock)
    callback: Arc<RwLock<Option<Arc<SamplingCallback>>>>,
    /// Optional MCP client for context injection
    client: Arc<RwLock<Option<Arc<McpClient>>>>,
}

impl SamplingHandler {
    /// Create a new sampling handler
    #[must_use]
    pub fn new() -> Self {
        Self {
            callback: Arc::new(RwLock::new(None)),
            client: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the MCP client for context injection
    pub async fn set_client(&self, client: Arc<McpClient>) {
        let mut c = self.client.write().await;
        *c = Some(client);
    }

    /// Set the callback for handling sampling requests
    ///
    /// The callback receives a `SamplingRequest` and should return a `SamplingResponse`.
    /// This is typically wired to the Thinker for LLM calls.
    pub async fn set_callback<F, Fut>(&self, callback: F)
    where
        F: Fn(SamplingRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<SamplingResponse>> + Send + 'static,
    {
        let mut cb = self.callback.write().await;
        *cb = Some(Arc::new(Box::new(move |req| Box::pin(callback(req)))));
    }

    /// Check if a callback is registered
    pub async fn has_callback(&self) -> bool {
        self.callback.read().await.is_some()
    }

    /// Handle an incoming sampling request from server
    ///
    /// This is called when we receive a `sampling/createMessage` request via SSE.
    ///
    /// # Arguments
    ///
    /// * `request_id` - The JSON-RPC request ID
    /// * `params` - The sampling request parameters
    /// * `requesting_server` - The name of the server making the request
    pub async fn handle_request(
        &self,
        request_id: Value,
        params: Value,
        requesting_server: &str,
    ) -> Result<SamplingResponse> {
        // Parse the request
        let mut request: SamplingRequest = serde_json::from_value(params)
            .map_err(|e| AlephError::IoError(format!("Failed to parse sampling request: {e}")))?;

        tracing::debug!(
            request_id = %request_id,
            message_count = request.messages.len(),
            "Processing sampling request"
        );

        // Inject context if requested
        if let Some(ref mode) = request.include_context {
            // Clone the client Arc under the lock, then release it before the
            // network round-trips in gather_context. Holding the read lock
            // across .await would block set_client writers for the duration of
            // the resource/tool listing (mirrors the callback clone below).
            // rust-doctor-disable-next-line excessive-clone
            let client = self.client.read().await.clone();
            if let Some(client) = client {
                let contexts =
                    ContextInjector::gather_context(&client, mode, requesting_server).await;
                if let Some(context_msg) = ContextInjector::format_as_system_message(&contexts) {
                    // Prepend context message to messages
                    request.messages.insert(0, context_msg);
                    tracing::debug!(
                        request_id = %request_id,
                        mode = ?mode,
                        context_count = contexts.len(),
                        "Injected context into sampling request"
                    );
                }
            }
        }

        // Clone callback ref under lock, then release before awaiting
        let cb = {
            let callback = self.callback.read().await;
            Arc::clone(callback.as_ref().ok_or_else(|| {
                AlephError::IoError("No sampling callback registered".to_string())
            })?)
        };

        // Invoke callback (lock released)
        let response = cb(request).await?;

        tracing::debug!(
            request_id = %request_id,
            "Sampling request completed"
        );

        Ok(response)
    }

    /// Create a simple text response
    #[cfg(test)]
    pub fn text_response(text: impl Into<String>) -> SamplingResponse {
        SamplingResponse {
            role: PromptRole::Assistant,
            content: SamplingContent::Text { text: text.into() },
            model: None,
            stop_reason: Some(StopReason::EndTurn),
        }
    }

    /// Create an error response (still valid `SamplingResponse` with error text)
    #[cfg(test)]
    pub fn error_response(error: impl Into<String>) -> SamplingResponse {
        SamplingResponse {
            role: PromptRole::Assistant,
            content: SamplingContent::Text {
                text: format!("Error: {}", error.into()),
            },
            model: None,
            stop_reason: Some(StopReason::EndTurn),
        }
    }
}

impl Default for SamplingHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampling_handler_creation() {
        let handler = SamplingHandler::new();
        // Should create without panicking
        drop(handler);
    }

    #[tokio::test]
    async fn test_has_callback_false() {
        let handler = SamplingHandler::new();
        assert!(!handler.has_callback().await);
    }

    #[tokio::test]
    async fn test_has_callback_true() {
        let handler = SamplingHandler::new();
        handler
            .set_callback(|_req| async { Ok(SamplingHandler::text_response("test")) })
            .await;
        assert!(handler.has_callback().await);
    }

    #[test]
    fn test_text_response() {
        let response = SamplingHandler::text_response("Hello, world!");
        assert!(matches!(response.content, SamplingContent::Text { .. }));
        assert!(matches!(response.stop_reason, Some(StopReason::EndTurn)));
        assert!(matches!(response.role, PromptRole::Assistant));
    }

    #[test]
    fn test_error_response() {
        let response = SamplingHandler::error_response("Something went wrong");
        if let SamplingContent::Text { text } = response.content {
            assert!(text.contains("Error:"));
            assert!(text.contains("Something went wrong"));
        } else {
            panic!("Expected text content");
        }
    }
}
