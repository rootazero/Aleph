/// OpenAI API types
///
/// Data structures for OpenAI chat completion requests and responses.

use serde::{Deserialize, Serialize};

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

/// Response from OpenAI chat completion API
#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
pub struct ResponseMessage {
    pub content: String,
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
