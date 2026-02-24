//! MCP Sampling Handler
//!
//! Handles server-initiated sampling/createMessage requests,
//! allowing MCP servers to call the host's LLM.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::client::McpClient;
use crate::mcp::context_injector::ContextInjector;
use crate::mcp::jsonrpc::mcp::{
    PromptRole, SamplingChunk, SamplingContent, SamplingMessage, SamplingRequest, SamplingResponse,
    StopReason,
};

/// Callback for handling sampling requests
///
/// Takes a SamplingRequest and returns a Future that resolves to a SamplingResponse.
pub type SamplingCallback = Box<
    dyn Fn(SamplingRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SamplingResponse>> + Send>>
        + Send
        + Sync,
>;

/// Callback for streaming sampling requests
pub type StreamingSamplingCallback = Box<
    dyn Fn(SamplingRequest) -> Pin<Box<dyn Stream<Item = Result<SamplingChunk>> + Send>>
        + Send
        + Sync,
>;

/// Manages sampling requests from MCP servers
pub struct SamplingHandler {
    /// Callback to invoke for sampling requests
    callback: Arc<RwLock<Option<SamplingCallback>>>,
    /// Streaming callback (optional, for streaming responses)
    streaming_callback: Arc<RwLock<Option<StreamingSamplingCallback>>>,
    /// Optional MCP client for context injection
    client: Arc<RwLock<Option<Arc<McpClient>>>>,
}

impl SamplingHandler {
    /// Create a new sampling handler
    pub fn new() -> Self {
        Self {
            callback: Arc::new(RwLock::new(None)),
            streaming_callback: Arc::new(RwLock::new(None)),
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
    /// The callback receives a SamplingRequest and should return a SamplingResponse.
    /// This is typically wired to the Thinker for LLM calls.
    pub async fn set_callback<F, Fut>(&self, callback: F)
    where
        F: Fn(SamplingRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<SamplingResponse>> + Send + 'static,
    {
        let mut cb = self.callback.write().await;
        *cb = Some(Box::new(move |req| Box::pin(callback(req))));
    }

    /// Check if a callback is registered
    pub async fn has_callback(&self) -> bool {
        self.callback.read().await.is_some()
    }

    /// Set streaming callback for streaming responses
    pub async fn set_streaming_callback<F, S>(&self, callback: F)
    where
        F: Fn(SamplingRequest) -> S + Send + Sync + 'static,
        S: Stream<Item = Result<SamplingChunk>> + Send + 'static,
    {
        let mut cb = self.streaming_callback.write().await;
        *cb = Some(Box::new(move |req| Box::pin(callback(req))));
    }

    /// Check if streaming is available
    pub async fn has_streaming(&self) -> bool {
        self.streaming_callback.read().await.is_some()
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
        request_id: u64,
        params: Value,
        requesting_server: &str,
    ) -> Result<SamplingResponse> {
        // Parse the request
        let mut request: SamplingRequest = serde_json::from_value(params).map_err(|e| {
            AlephError::IoError(format!("Failed to parse sampling request: {}", e))
        })?;

        tracing::debug!(
            request_id = request_id,
            message_count = request.messages.len(),
            "Processing sampling request"
        );

        // Inject context if requested
        if let Some(ref mode) = request.include_context {
            if let Some(ref client) = *self.client.read().await {
                let contexts =
                    ContextInjector::gather_context(client, mode, requesting_server).await;
                if let Some(context_msg) = ContextInjector::format_as_system_message(&contexts) {
                    // Prepend context message to messages
                    request.messages.insert(0, context_msg);
                    tracing::debug!(
                        request_id = request_id,
                        mode = ?mode,
                        context_count = contexts.len(),
                        "Injected context into sampling request"
                    );
                }
            }
        }

        // Get callback
        let callback = self.callback.read().await;
        let cb = callback.as_ref().ok_or_else(|| {
            AlephError::IoError("No sampling callback registered".to_string())
        })?;

        // Invoke callback
        let response = cb(request).await?;

        tracing::debug!(
            request_id = request_id,
            "Sampling request completed"
        );

        Ok(response)
    }

    /// Create a simple text response
    pub fn text_response(text: impl Into<String>) -> SamplingResponse {
        SamplingResponse {
            role: PromptRole::Assistant,
            content: SamplingContent::Text { text: text.into() },
            model: None,
            stop_reason: Some(StopReason::EndTurn),
        }
    }

    /// Create an error response (still valid SamplingResponse with error text)
    pub fn error_response(error: impl Into<String>) -> SamplingResponse {
        SamplingResponse {
            role: PromptRole::Assistant,
            content: SamplingContent::Text {
                text: format!("Error: {}", error.into())
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

/// Convert SamplingMessage to a format suitable for Thinker
///
/// Returns a vector of (role, content) tuples.
pub fn sampling_messages_to_chat(messages: &[SamplingMessage]) -> Vec<(String, String)> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                PromptRole::User => "user",
                PromptRole::Assistant => "assistant",
                PromptRole::System => "system",
            };
            let content = match &m.content {
                SamplingContent::Text { text } => text.clone(),
                SamplingContent::Image { data, mime_type } => {
                    format!("[Image: {} ({} bytes)]", mime_type, data.len())
                }
            };
            (role.to_string(), content)
        })
        .collect()
}

/// Extract system prompt from sampling request
pub fn extract_system_prompt(request: &SamplingRequest) -> Option<String> {
    // First check explicit system_prompt field
    if let Some(ref system) = request.system_prompt {
        return Some(system.clone());
    }

    // Then check for system role in messages
    for msg in &request.messages {
        if matches!(msg.role, PromptRole::System) {
            if let SamplingContent::Text { ref text } = msg.content {
                return Some(text.clone());
            }
        }
    }

    None
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
        handler.set_callback(|_req| async {
            Ok(SamplingHandler::text_response("test"))
        }).await;
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

    #[test]
    fn test_messages_to_chat() {
        let messages = vec![
            SamplingMessage {
                role: PromptRole::User,
                content: SamplingContent::Text { text: "Hello".to_string() },
            },
            SamplingMessage {
                role: PromptRole::Assistant,
                content: SamplingContent::Text { text: "Hi there!".to_string() },
            },
        ];

        let chat = sampling_messages_to_chat(&messages);
        assert_eq!(chat.len(), 2);
        assert_eq!(chat[0].0, "user");
        assert_eq!(chat[0].1, "Hello");
        assert_eq!(chat[1].0, "assistant");
        assert_eq!(chat[1].1, "Hi there!");
    }

    #[test]
    fn test_extract_system_prompt_from_field() {
        let request = SamplingRequest {
            messages: vec![],
            model_preferences: None,
            system_prompt: Some("You are a helpful assistant.".to_string()),
            include_context: None,
            max_tokens: None,
        };

        let system = extract_system_prompt(&request);
        assert_eq!(system, Some("You are a helpful assistant.".to_string()));
    }

    #[test]
    fn test_extract_system_prompt_from_messages() {
        let request = SamplingRequest {
            messages: vec![
                SamplingMessage {
                    role: PromptRole::System,
                    content: SamplingContent::Text { text: "System message".to_string() },
                },
                SamplingMessage {
                    role: PromptRole::User,
                    content: SamplingContent::Text { text: "Hello".to_string() },
                },
            ],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            max_tokens: None,
        };

        let system = extract_system_prompt(&request);
        assert_eq!(system, Some("System message".to_string()));
    }

    #[test]
    fn test_extract_system_prompt_none() {
        let request = SamplingRequest {
            messages: vec![
                SamplingMessage {
                    role: PromptRole::User,
                    content: SamplingContent::Text { text: "Hello".to_string() },
                },
            ],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            max_tokens: None,
        };

        let system = extract_system_prompt(&request);
        assert!(system.is_none());
    }
}
