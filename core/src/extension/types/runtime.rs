//! Runtime interaction types for plugin extensions
//!
//! This module contains types for plugin runtime interactions including:
//! - Background services (lifecycle management)
//! - Messaging channels (external platform integration)
//! - AI providers (custom model providers)
//! - HTTP routes (plugin-provided endpoints)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Service Types (V2 Background Services)
// =============================================================================

/// Service state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ServiceState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}


/// Running service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub state: ServiceState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

/// Service lifecycle result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResult {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

impl ServiceResult {
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            data: None,
        }
    }

    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(msg.into()),
            data: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(msg.into()),
            data: None,
        }
    }
}

// =============================================================================
// Channel Types (V2 Plugin Channels)
// =============================================================================

/// Channel message from external platform
///
/// Represents an incoming message from a plugin-provided messaging channel
/// (e.g., Telegram, Discord, Slack, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    /// Unique channel identifier (e.g., "telegram", "discord")
    pub channel_id: String,
    /// Conversation/chat identifier within the channel
    pub conversation_id: String,
    /// Sender identifier (user ID on the platform)
    pub sender_id: String,
    /// Message content
    pub content: String,
    /// When the message was sent
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Platform-specific metadata (e.g., message_id, attachments)
    pub metadata: Option<serde_json::Value>,
}

/// Channel send request
///
/// Request to send a message through a plugin-provided channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSendRequest {
    /// Conversation/chat identifier to send to
    pub conversation_id: String,
    /// Message content to send
    pub content: String,
    /// Optional message ID to reply to
    pub reply_to: Option<String>,
    /// Platform-specific options (e.g., parse_mode, disable_notification)
    pub metadata: Option<serde_json::Value>,
}

/// Channel connection state
///
/// Represents the current connection status of a plugin channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ChannelState {
    /// Channel is not connected
    #[default]
    Disconnected,
    /// Channel is attempting to connect
    Connecting,
    /// Channel is connected and operational
    Connected,
    /// Channel lost connection and is attempting to reconnect
    Reconnecting,
    /// Channel connection failed
    Failed,
}


/// Channel info
///
/// Describes a plugin-provided messaging channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    /// Unique channel identifier
    pub id: String,
    /// Plugin that provides this channel
    pub plugin_id: String,
    /// Human-readable label (e.g., "Telegram Bot")
    pub label: String,
    /// Current connection state
    pub state: ChannelState,
    /// Error message if state is Failed
    pub error: Option<String>,
}

// =============================================================================
// Provider Types (V2 Plugin Providers)
// =============================================================================

/// Provider chat request
///
/// Represents a chat completion request to a plugin-provided AI model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatRequest {
    /// Model identifier (e.g., "gpt-4", "claude-3-opus")
    pub model: String,
    /// Conversation messages
    pub messages: Vec<ProviderMessage>,
    /// Sampling temperature (0.0 - 2.0)
    pub temperature: Option<f32>,
    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,
    /// Whether to stream the response
    pub stream: bool,
}

/// Provider message
///
/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMessage {
    /// Message role (e.g., "system", "user", "assistant")
    pub role: String,
    /// Message content
    pub content: String,
}

/// Provider chat response (non-streaming)
///
/// Complete response from a chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatResponse {
    /// Generated response content
    pub content: String,
    /// Reason the generation stopped (e.g., "stop", "length", "tool_calls")
    pub finish_reason: Option<String>,
    /// Token usage statistics
    pub usage: Option<ProviderUsage>,
}

/// Provider usage info
///
/// Token usage statistics for a completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    /// Number of tokens in the prompt
    pub prompt_tokens: u32,
    /// Number of tokens in the completion
    pub completion_tokens: u32,
    /// Total tokens used (prompt + completion)
    pub total_tokens: u32,
}

/// Provider streaming chunk
///
/// A chunk of data in a streaming response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderStreamChunk {
    /// Content delta - partial response text
    #[serde(rename = "delta")]
    Delta { content: String },
    /// Stream completed
    #[serde(rename = "done")]
    Done { usage: Option<ProviderUsage> },
    /// Error occurred during streaming
    #[serde(rename = "error")]
    Error { message: String },
}

/// Provider model info
///
/// Describes a model available from a plugin-provided AI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelInfo {
    /// Model identifier
    pub id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Context window size in tokens
    pub context_window: Option<u32>,
    /// Whether the model supports tool/function calling
    pub supports_tools: bool,
    /// Whether the model supports vision/image inputs
    pub supports_vision: bool,
}

// =============================================================================
// HTTP Route Types (V2 Plugin HTTP Endpoints)
// =============================================================================

/// HTTP request from plugin route
///
/// Represents an incoming HTTP request to a plugin-provided endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    /// HTTP method (e.g., "GET", "POST", "PUT", "DELETE")
    pub method: String,
    /// Request path (e.g., "/api/webhook")
    pub path: String,
    /// HTTP headers as key-value pairs
    pub headers: HashMap<String, String>,
    /// Query string parameters
    pub query: HashMap<String, String>,
    /// Request body (for POST/PUT/PATCH requests)
    pub body: Option<serde_json::Value>,
    /// Path parameters extracted from route patterns (e.g., ":id" -> "123")
    pub path_params: HashMap<String, String>,
}

/// HTTP response from plugin handler
///
/// Response to send back to the HTTP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code (e.g., 200, 404, 500)
    pub status: u16,
    /// HTTP response headers
    pub headers: HashMap<String, String>,
    /// Response body
    pub body: Option<serde_json::Value>,
}

impl HttpResponse {
    /// Create a 200 OK response with no body
    pub fn ok() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: None,
        }
    }

    /// Create a 200 OK response with JSON body
    pub fn json(data: serde_json::Value) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        Self {
            status: 200,
            headers,
            body: Some(data),
        }
    }

    /// Create an error response with the given status code and message
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: Some(serde_json::json!({"error": message.into()})),
        }
    }

    /// Create a 404 Not Found response
    pub fn not_found() -> Self {
        Self::error(404, "Not Found")
    }

    /// Create a 400 Bad Request response
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::error(400, message)
    }

    /// Create a 500 Internal Server Error response
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::error(500, message)
    }
}
