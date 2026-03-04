//! ChatGPT backend-api request/response types

use serde::{Deserialize, Serialize};

/// ChatGPT backend-api conversation request
#[derive(Debug, Serialize)]
pub struct ChatGptRequest {
    pub action: String,
    pub messages: Vec<ChatGptMessage>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub parent_message_id: String,
    pub timezone_offset_min: i32,
    pub conversation_mode: ConversationMode,
}

/// A message in the ChatGPT conversation
#[derive(Debug, Serialize)]
pub struct ChatGptMessage {
    pub id: String,
    pub author: Author,
    pub content: ChatGptContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Message author
#[derive(Debug, Serialize)]
pub struct Author {
    pub role: String,
}

/// Message content
#[derive(Debug, Serialize)]
pub struct ChatGptContent {
    pub content_type: String,
    pub parts: Vec<serde_json::Value>,
}

/// Conversation mode controls built-in tools
#[derive(Debug, Serialize)]
pub struct ConversationMode {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_ids: Option<Vec<String>>,
}

/// ChatGPT SSE response message wrapper
#[derive(Debug, Deserialize)]
pub struct ChatGptStreamResponse {
    pub message: Option<ChatGptResponseMessage>,
    pub conversation_id: Option<String>,
    pub error: Option<serde_json::Value>,
}

/// Response message from ChatGPT
#[derive(Debug, Deserialize)]
pub struct ChatGptResponseMessage {
    pub id: String,
    pub author: ResponseAuthor,
    pub content: ResponseContent,
    #[serde(default)]
    pub status: String,
}

/// Response author
#[derive(Debug, Deserialize)]
pub struct ResponseAuthor {
    pub role: String,
}

/// Response content
#[derive(Debug, Deserialize)]
pub struct ResponseContent {
    pub content_type: String,
    #[serde(default)]
    pub parts: Vec<serde_json::Value>,
}

/// ChatGPT available models response
#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
}

/// Model information
#[derive(Debug, Deserialize)]
pub struct ModelInfo {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Chat requirements response (security tokens)
#[derive(Debug, Deserialize)]
pub struct ChatRequirements {
    pub token: String,
    #[serde(default)]
    pub proofofwork: Option<ProofOfWork>,
}

/// Proof-of-work challenge
#[derive(Debug, Deserialize)]
pub struct ProofOfWork {
    pub required: bool,
    pub seed: Option<String>,
    pub difficulty: Option<String>,
}
