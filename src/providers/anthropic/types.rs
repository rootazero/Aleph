// core/src/providers/anthropic/types.rs

//! Anthropic API types
//!
//! Request and response structures for Claude Messages API.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::thinker::cache::CacheControl;

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
    /// `top_p` nucleus sampling. Capability-gated (`supports_top_p`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// `top_k` sampling. Capability-gated (`supports_top_k`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Anthropic `stop_sequences` (up to 4 sequences). Capability-gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
    /// Service tier for priority or batch processing (e.g. "auto", "flex")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Anthropic abuse-detection / rate-limit metadata.
    /// Capability-gated (`supports_metadata_user_id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// Output configuration (effort level, structured output format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

/// Anthropic abuse-detection / rate-limit metadata.
/// Spec: <https://docs.anthropic.com/en/api/messages#body-metadata>
#[derive(Debug, Serialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// System prompt block (array format for compatibility)
#[derive(Debug, Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    /// Optional cache control marker for Anthropic prompt caching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl SystemBlock {
    /// Create a text system block (no caching)
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            block_type: "text".to_string(),
            text: content.into(),
            cache_control: None,
        }
    }

    /// Create a text system block with ephemeral cache control
    pub fn cached_text(content: impl Into<String>) -> Self {
        Self {
            block_type: "text".to_string(),
            text: content.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}

/// Extended thinking configuration.
///
/// - `thinking_type`: "enabled" | "disabled" | "adaptive"
/// - `budget_tokens`: Required for "enabled", optional for "adaptive"
/// - `display`: "summarized" (default) | "omitted"
#[derive(Debug, Serialize)]
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

/// Output configuration for controlling response quality and format.
///
/// Wired from `ProviderConfig.effort` in `build_request` (Cycle 4).
#[derive(Debug, Serialize)]
pub struct OutputConfig {
    /// Output effort level: "low", "medium", "high", "max"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Image content (base64)
    Image { source: ImageSource },
    /// Extended-thinking block from a prior assistant turn.
    ///
    /// Anthropic requires a signed thinking block to be replayed verbatim when
    /// the same turn also contains `tool_use` blocks; the signature is opaque
    /// and must round-trip exactly.
    Thinking { thinking: String, signature: String },
    /// Tool use (assistant requesting tool execution)
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (user returning tool output)
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
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
/// This enum handles text, thinking, and `tool_use` block types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
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
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
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
            AnthropicContentBlock::Thinking { thinking, .. } => {
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
    fn test_system_block_with_cache_control_serializes() {
        let block = SystemBlock {
            block_type: "text".into(),
            text: "You are a helpful assistant.".into(),
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("cache_control"));
        assert!(json.contains("ephemeral"));
    }

    #[test]
    fn test_system_block_without_cache_control_omits_field() {
        let block = SystemBlock {
            block_type: "text".into(),
            text: "You are a helpful assistant.".into(),
            cache_control: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(!json.contains("cache_control"));
    }

    #[test]
    fn test_system_block_cached_text_constructor() {
        let block = SystemBlock::cached_text("Stable system prompt");
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("cache_control"));
        assert!(json.contains("ephemeral"));
        assert!(json.contains("Stable system prompt"));
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

    #[test]
    fn anthropic_usage_parses_cache_creation_input_tokens() {
        let json = serde_json::json!({
            "input_tokens": 200,
            "output_tokens": 100,
            "cache_read_input_tokens": 150,
            "cache_creation_input_tokens": 50
        });
        let usage: AnthropicUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.cache_creation_input_tokens, Some(50));
    }

    #[test]
    fn messages_request_serializes_with_all_cycle4_fields() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet".to_string(),
            messages: vec![],
            max_tokens: 1024,
            system: None,
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            stop_sequences: Some(vec!["END".to_string(), "STOP".to_string()]),
            stream: Some(true),
            thinking: None,
            tools: None,
            service_tier: Some("auto".to_string()),
            metadata: Some(Metadata {
                user_id: Some("u_42".to_string()),
            }),
            output_config: Some(OutputConfig {
                effort: Some("high".to_string()),
            }),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-3-5-sonnet");
        assert_eq!(json["max_tokens"], 1024);
        assert!((json["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-4);
        assert!((json["top_p"].as_f64().unwrap() - 0.9).abs() < 1e-4);
        assert_eq!(json["top_k"], 40);
        assert_eq!(json["stop_sequences"], serde_json::json!(["END", "STOP"]));
        assert_eq!(json["stream"], true);
        assert_eq!(json["service_tier"], "auto");
        assert_eq!(json["metadata"]["user_id"], "u_42");
        assert_eq!(json["output_config"]["effort"], "high");
    }

    #[test]
    fn messages_request_omits_none_cycle4_fields_on_wire() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet".to_string(),
            messages: vec![],
            max_tokens: 1024,
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            thinking: None,
            tools: None,
            service_tier: None,
            metadata: None,
            output_config: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("top_p").is_none());
        assert!(json.get("top_k").is_none());
        assert!(json.get("stop_sequences").is_none());
        assert!(json.get("metadata").is_none());
        assert!(json.get("service_tier").is_none());
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn metadata_omits_user_id_when_none() {
        let m = Metadata { user_id: None };
        let json = serde_json::to_value(&m).unwrap();
        // Object exists but has no fields
        assert_eq!(json, serde_json::json!({}));
    }
}
