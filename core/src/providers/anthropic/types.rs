// core/src/providers/anthropic/types.rs

//! Anthropic API types
//!
//! Request and response structures for Claude Messages API.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request body for Claude Messages API
#[derive(Debug, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
}

/// System prompt block (array format for compatibility)
#[derive(Debug, Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
}

impl SystemBlock {
    /// Create a text system block
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            block_type: "text".to_string(),
            text: content.into(),
        }
    }
}

/// Extended thinking configuration
#[derive(Debug, Serialize)]
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub thinking_type: String,
    pub budget_tokens: u32,
}

/// Message structure
#[derive(Debug, Serialize)]
pub struct Message {
    pub role: String,
    #[serde(flatten)]
    pub content: MessageContent,
}

/// Message content variants
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text message
    Text { content: String },
    /// Multimodal message with content blocks
    Multimodal { content: Vec<ContentBlock> },
}

/// Content block for multimodal messages
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content
    Text { text: String },
    /// Image content (base64)
    Image { source: ImageSource },
}

/// Image source for base64 encoded images
#[derive(Debug, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// Response from Messages API
#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    pub content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<AnthropicUsage>,
}

/// Tool definition for Anthropic API
#[derive(Debug, Clone, Serialize)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Content block in Anthropic response (tagged union)
///
/// Anthropic API returns an array of content blocks, each with a "type" field.
/// This enum handles text, thinking, and tool_use block types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

/// Usage info from Anthropic API
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
}

/// Error response
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
}

#[derive(Debug, Deserialize)]
pub struct ErrorDetails {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_tool_serialization() {
        let tool = AnthropicTool {
            name: "search".into(),
            description: "Search the web".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "search");
        assert_eq!(json["description"], "Search the web");
        assert!(json["input_schema"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_content_block_text_deserialization() {
        let json = r#"{"type": "text", "text": "Hello world"}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();
        match block {
            AnthropicContentBlock::Text { text } => assert_eq!(text, "Hello world"),
            _ => panic!("Expected Text block"),
        }
    }

    #[test]
    fn test_content_block_thinking_deserialization() {
        let json = r#"{"type": "thinking", "thinking": "Let me reason about this..."}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();
        match block {
            AnthropicContentBlock::Thinking { thinking } => {
                assert_eq!(thinking, "Let me reason about this...");
            }
            _ => panic!("Expected Thinking block"),
        }
    }

    #[test]
    fn test_content_block_tool_use_deserialization() {
        let json = r#"{
            "type": "tool_use",
            "id": "toolu_123",
            "name": "search",
            "input": {"query": "rust programming"}
        }"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();
        match block {
            AnthropicContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "search");
                assert_eq!(input["query"], "rust programming");
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_parse_tool_use_response() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Let me search for that."},
                {"type": "tool_use", "id": "toolu_123", "name": "search", "input": {"query": "rust"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert!(usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn test_parse_text_only_response() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_parse_response_with_usage_and_cache() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 200, "output_tokens": 100, "cache_read_input_tokens": 150}
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.cache_read_input_tokens, Some(150));
    }

    #[test]
    fn test_parse_mixed_content_blocks() {
        let json = r#"{
            "content": [
                {"type": "thinking", "thinking": "Reasoning step..."},
                {"type": "text", "text": "Here is my answer."},
                {"type": "tool_use", "id": "toolu_456", "name": "read_file", "input": {"path": "/tmp/test.rs"}}
            ],
            "stop_reason": "tool_use"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 3);
    }
}
