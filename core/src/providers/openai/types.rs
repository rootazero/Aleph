/// OpenAI API types
///
/// Data structures for OpenAI chat completion requests and responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request body for OpenAI chat completion API
#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Reasoning effort for o1/o3 models: "low", "medium", "high"
    /// Only applicable to models that support extended thinking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// Message format for chat API
///
/// Supports both text-only and multimodal (text + image) messages.
#[derive(Debug, Serialize)]
pub struct Message {
    pub role: String,
    #[serde(flatten)]
    pub content: MessageContent,
}

/// Message content can be either simple text or structured content blocks
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text message
    Text { content: String },
    /// Multimodal message with text and/or images
    Multimodal { content: Vec<ContentBlock> },
}

/// Content block for multimodal messages
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content block
    Text { text: String },
    /// Image URL content block (supports data URIs)
    ImageUrl { image_url: ImageUrl },
}

/// Image URL wrapper
#[derive(Debug, Serialize)]
pub struct ImageUrl {
    pub url: String,
    /// Detail level for image processing: "low", "high", or "auto"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// =============================================================================
// Tool Types (for function calling)
// =============================================================================

/// Tool definition for OpenAI API function calling
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub tool_type: String, // always "function"
    pub function: OpenAiFunction,
}

/// Function definition within an OpenAI tool
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// When true, the model must strictly follow the JSON Schema.
    /// Requires all properties to be listed in `required` and
    /// `additionalProperties: false` on every object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// Tool call in OpenAI API response
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiToolCall {
    pub id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)] // Deserialized from API response
    pub call_type: Option<String>,
    pub function: OpenAiFunctionCall,
}

/// Function call details within a tool call
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiFunctionCall {
    pub name: String,
    /// JSON string — needs parsing via serde_json::from_str()
    pub arguments: String,
}

// =============================================================================
// Response Types
// =============================================================================

/// Response from OpenAI chat completion API
#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<Choice>,
    pub usage: Option<OpenAiUsage>,
}

/// Token usage statistics from OpenAI API
#[derive(Debug, Deserialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[allow(dead_code)] // Deserialized from API response
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: ResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResponseMessage {
    /// Nullable — can be null when tool_calls are present
    pub content: Option<String>,
    /// Tool calls from the model (present when model invokes functions)
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}

/// Error response from OpenAI API
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
}

#[derive(Debug, Deserialize)]
pub struct ErrorDetails {
    pub message: String,
    #[serde(rename = "type")]
    #[allow(dead_code)] // Deserialized from API response
    pub error_type: String,
}
