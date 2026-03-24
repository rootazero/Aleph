//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason,
    TokenUsage,
};
use crate::providers::anthropic::{
    AnthropicContentBlock, AnthropicTool, ContentBlock, ErrorResponse, ImageSource, Message,
    MessageContent, MessagesRequest, MessagesResponse, SystemBlock, ThinkingBlock,
};
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::Client;
use tracing::{debug, error};

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic protocol adapter
pub struct AnthropicProtocol {
    client: Client,
}

impl AnthropicProtocol {
    /// Create a new Anthropic protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL
    fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        // Normalize URL
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        format!("{}/v1/messages", base_url)
    }

    /// Convert UnifiedMessages to Anthropic Messages
    fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Message> {
        let mut result = Vec::new();
        let mut i = 0;
        while i < messages.len() {
            match &messages[i] {
                UnifiedMessage::User { content } => {
                    let mut blocks = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text } => {
                                blocks.push(ContentBlock::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::Image { data, mime_type } => {
                                blocks.push(ContentBlock::Image {
                                    source: ImageSource {
                                        source_type: "base64".to_string(),
                                        media_type: mime_type.clone(),
                                        data: data.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    let image_count = blocks.iter().filter(|b| matches!(b, ContentBlock::Image { .. })).count();
                    if image_count > 0 {
                        tracing::info!(
                            target: "multimodal",
                            probe = "P6_provider",
                            role = "user",
                            content_type = "multimodal",
                            image_count = image_count,
                            "Anthropic multimodal message converted"
                        );
                    }
                    if blocks.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: String::new(),
                        });
                    }
                    if blocks.len() == 1 {
                        if let ContentBlock::Text { text } = &blocks[0] {
                            result.push(Message {
                                role: "user".to_string(),
                                content: MessageContent::Text {
                                    content: text.clone(),
                                },
                            });
                            i += 1;
                            continue;
                        }
                    }
                    result.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal { content: blocks },
                    });
                    i += 1;
                }
                UnifiedMessage::Assistant { content } => {
                    let mut blocks = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text } => {
                                blocks.push(ContentBlock::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                // Sanitize tool_use_id for Anthropic
                                let sanitized_id: String = id
                                    .chars()
                                    .map(|c| {
                                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                            c
                                        } else {
                                            '_'
                                        }
                                    })
                                    .take(64)
                                    .collect();
                                blocks.push(ContentBlock::ToolUse {
                                    id: sanitized_id,
                                    name: name.clone(),
                                    input: arguments.clone(),
                                });
                            }
                            _ => {}
                        }
                    }
                    if blocks.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: String::new(),
                        });
                    }
                    result.push(Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Multimodal { content: blocks },
                    });
                    i += 1;
                }
                UnifiedMessage::ToolResult { .. } => {
                    // Collect consecutive ToolResults into one user message
                    let mut tool_blocks = Vec::new();
                    while i < messages.len() {
                        if let UnifiedMessage::ToolResult {
                            tool_call_id,
                            content,
                            is_error,
                            ..
                        } = &messages[i]
                        {
                            let output = content
                                .iter()
                                .map(|b| match b {
                                    crate::providers::message::ContentBlock::Text { text } => {
                                        text.clone()
                                    }
                                    crate::providers::message::ContentBlock::Json { value } => {
                                        serde_json::to_string(value).unwrap_or_default()
                                    }
                                    _ => String::new(),
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            // Sanitize tool_use_id
                            let sanitized_id: String = tool_call_id
                                .chars()
                                .map(|c| {
                                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                        c
                                    } else {
                                        '_'
                                    }
                                })
                                .take(64)
                                .collect();
                            tool_blocks.push(ContentBlock::ToolResult {
                                tool_use_id: sanitized_id,
                                content: output,
                                is_error: *is_error,
                            });
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    result.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal {
                            content: tool_blocks,
                        },
                    });
                }
            }
        }
        result
    }

    /// Map ThinkLevel to budget_tokens
    fn map_think_level(level: &ThinkLevel) -> Option<u32> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(1024),
            ThinkLevel::Low => Some(4096),
            ThinkLevel::Medium => Some(10000),
            ThinkLevel::High => Some(20000),
            ThinkLevel::XHigh => Some(50000),
        }
    }

    /// Parse SSE line for streaming
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }

        let data = &line[6..];
        if data == "[DONE]" {
            return None;
        }

        let parsed: serde_json::Value = serde_json::from_str(data).ok()?;

        // Handle content_block_delta events
        if parsed.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
            return parsed["delta"]["text"].as_str().map(|s| s.to_string());
        }

        None
    }
}

#[async_trait]
impl ProtocolAdapter for AnthropicProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let messages = Self::convert_messages(payload.messages);

        // Per-request overrides provider config
        let max_tokens = payload.max_tokens.or(config.max_tokens).unwrap_or(DEFAULT_MAX_TOKENS);
        let temperature = payload.temperature.or(config.temperature);

        // Build thinking config if enabled
        let thinking = payload
            .think_level
            .as_ref()
            .and_then(Self::map_think_level)
            .map(|budget| ThinkingBlock {
                thinking_type: "enabled".to_string(),
                budget_tokens: budget,
            });

        // Convert tool definitions to Anthropic format
        let tools = payload.tools.map(|tool_defs| {
            tool_defs
                .iter()
                .map(|td| {
                    // Ensure input_schema has "type" field — required by strict
                    // backends like AWS Bedrock, which rejects schemas without it.
                    let mut schema = td.parameters.clone();
                    if let Some(obj) = schema.as_object_mut() {
                        obj.entry("type").or_insert_with(|| serde_json::json!("object"));
                    }
                    // Migrate schemars draft-07 schemas to draft 2020-12
                    crate::tools::schema_strictify::migrate_to_draft_2020_12(&mut schema);
                    AnthropicTool {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        input_schema: schema,
                    }
                })
                .collect()
        });

        let request_body = MessagesRequest {
            model: config.default_model().to_string(),
            messages,
            max_tokens,
            system: payload.system_prompt.map(|s| vec![SystemBlock::text(s)]),
            temperature,
            stream: if is_streaming { Some(true) } else { None },
            thinking,
            tools,
        };

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.default_model(),
            streaming = is_streaming,
            "Building Anthropic request"
        );

        // Serialize to JSON value so we can add tool_choice if needed
        let mut body = serde_json::to_value(&request_body)
            .map_err(|e| AlephError::provider(format!("Failed to serialize request: {}", e)))?;

        // Add tool_choice if specified
        if let Some(ref choice) = payload.tool_choice {
            use crate::providers::adapter::ToolChoice;
            match choice {
                ToolChoice::Auto => { body["tool_choice"] = serde_json::json!({"type": "auto"}); }
                ToolChoice::Required => { body["tool_choice"] = serde_json::json!({"type": "any"}); }
                ToolChoice::Specific(name) => {
                    body["tool_choice"] = serde_json::json!({"type": "tool", "name": name});
                }
                ToolChoice::None => {
                    // Anthropic: remove tools array entirely to disable tool use
                    if let Some(obj) = body.as_object_mut() {
                        obj.remove("tools");
                    }
                }
            }
        }

        Ok(self
            .client
            .post(&endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .json(&body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&error_text) {
                let msg = error_response.error.message;
                return match status.as_u16() {
                    401 => Err(AlephError::authentication("Anthropic", &msg)),
                    429 => Err(AlephError::rate_limit(format!("Anthropic: {}", msg))),
                    _ => Err(AlephError::provider(format!("Anthropic error: {}", msg))),
                };
            }

            return Err(AlephError::provider(format!(
                "Anthropic error ({}): {}",
                status, error_text
            )));
        }

        let response_body: MessagesResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse Anthropic response");
            AlephError::provider(format!("Failed to parse response: {}", e))
        })?;

        let mut provider_response = ProviderResponse::default();

        for block in &response_body.content {
            match block {
                AnthropicContentBlock::Text { text } => {
                    // Append text (there may be multiple text blocks)
                    match &mut provider_response.text {
                        Some(existing) => {
                            existing.push_str(text);
                        }
                        None => {
                            provider_response.text = Some(text.clone());
                        }
                    }
                }
                AnthropicContentBlock::Thinking { thinking } => {
                    provider_response.thinking = Some(thinking.clone());
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    provider_response.tool_calls.push(NativeToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.clone(),
                    });
                }
            }
        }

        provider_response.stop_reason = match response_body.stop_reason.as_deref() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            _ => StopReason::Unknown,
        };

        if let Some(usage) = response_body.usage {
            provider_response.usage = Some(TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_input_tokens,
            });
        }

        Ok(provider_response)
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Anthropic streaming error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map(move |chunk| {
                let bytes = chunk.map_err(|e| AlephError::network(e.to_string()))?;
                let text = String::from_utf8_lossy(&bytes);

                let mut result = String::new();
                for line in text.lines() {
                    if let Some(content) = Self::parse_sse_line(line) {
                        result.push_str(&content);
                    }
                }

                Ok(result)
            })
            .filter(|r| {
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                std::future::ready(keep)
            })
            .boxed();

        Ok(stream)
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("claude-3-5-sonnet");
        let endpoint = AnthropicProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.base_url = Some("https://custom.api.com/v1".to_string());
        let endpoint = AnthropicProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://custom.api.com/v1/messages");
    }

    #[test]
    fn test_map_think_level() {
        assert_eq!(AnthropicProtocol::map_think_level(&ThinkLevel::Off), None);
        assert_eq!(
            AnthropicProtocol::map_think_level(&ThinkLevel::Medium),
            Some(10000)
        );
        assert_eq!(
            AnthropicProtocol::map_think_level(&ThinkLevel::High),
            Some(20000)
        );
    }

    #[test]
    fn test_parse_sse_content_block_delta() {
        let line = r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#;
        let result = AnthropicProtocol::parse_sse_line(line);
        assert_eq!(result, Some("Hello".to_string()));
    }

    #[test]
    fn test_parse_sse_done() {
        let line = "data: [DONE]";
        let result = AnthropicProtocol::parse_sse_line(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_supports_native_tools() {
        let protocol = AnthropicProtocol::new(Client::new());
        assert!(protocol.supports_native_tools());
    }

    #[test]
    fn test_build_request_includes_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

        let protocol = AnthropicProtocol::new(Client::new());
        let tools = vec![ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
            ToolCategory::Builtin,
        )];
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config, false).unwrap();
        let built = request.build().unwrap();

        // Verify the body contains tools
        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "search");
        assert_eq!(body["tools"][0]["description"], "Search the web");
        assert!(body["tools"][0]["input_schema"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_build_request_no_tools_when_none() {
        use crate::providers::message::UnifiedMessage;
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config, false).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        // tools field should be absent (skip_serializing_if = "Option::is_none")
        assert!(body.get("tools").is_none());
    }

    // =========================================================================
    // convert_messages() Tests
    // =========================================================================

    #[test]
    fn test_convert_s1_pure_text_user() {
        let msgs = [UnifiedMessage::user("Hello, Claude!")];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        // Single text user message uses Text variant (not Multimodal)
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello, Claude!");
    }

    #[test]
    fn test_convert_s2_multi_turn() {
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems programming language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "user");
    }

    #[test]
    fn test_convert_s3_assistant_text_and_tool_call() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::Text {
                    text: "Let me search for that.".to_string(),
                },
                CB::ToolCall {
                    id: "toolu_123".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                },
            ],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Let me search for that.");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["name"], "search");
        assert_eq!(content[1]["id"], "toolu_123");
        assert_eq!(content[1]["input"]["query"], "rust");
    }

    #[test]
    fn test_convert_s4_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "toolu_123",
            "search",
            "Found 3 results about Rust.",
            false,
        )];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "toolu_123");
        assert_eq!(content[0]["content"], "Found 3 results about Rust.");
        // is_error is false, should be omitted (skip_serializing_if)
        assert!(content[0].get("is_error").is_none());
    }

    #[test]
    fn test_convert_s5_full_cycle() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [
            UnifiedMessage::user("Search for Rust"),
            UnifiedMessage::Assistant {
                content: vec![CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "Rust"}),
                }],
            },
            UnifiedMessage::tool_result("call_1", "search", "Rust is great", false),
            UnifiedMessage::assistant("Based on the results, Rust is great!"),
        ];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "user"); // tool_result wrapped as user
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_convert_s6_multiple_tool_calls() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "rust"}),
                },
                CB::ToolCall {
                    id: "call_2".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/tmp/a.rs"}),
                },
            ],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "search");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["name"], "read_file");
    }

    #[test]
    fn test_convert_s7_consecutive_tool_results_merge() {
        let msgs = [
            UnifiedMessage::tool_result("call_1", "search", "result 1", false),
            UnifiedMessage::tool_result("call_2", "read_file", "result 2", false),
        ];
        let result = AnthropicProtocol::convert_messages(&msgs);

        // Consecutive ToolResults should merge into ONE user message
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[1]["type"], "tool_result");
        assert_eq!(content[1]["tool_use_id"], "call_2");
    }

    #[test]
    fn test_convert_s8_error_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "call_err",
            "search",
            "Connection timed out",
            true,
        )];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["content"], "Connection timed out");
        assert_eq!(content[0]["is_error"], true);
    }

    #[test]
    fn test_convert_s9_tool_id_sanitization() {
        use crate::providers::message::ContentBlock as CB;
        let long_special_id = "call/foo@bar#1!!!!".to_string();
        let msgs = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: long_special_id,
                name: "test".to_string(),
                arguments: serde_json::json!({}),
            }],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        let id = content[0]["id"].as_str().unwrap();
        // Special chars replaced with '_'
        assert_eq!(id, "call_foo_bar_1____");
        // No special chars remain
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));

        // Also test max 64 char truncation
        let long_id = "a".repeat(100);
        let msgs2 = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: long_id,
                name: "test".to_string(),
                arguments: serde_json::json!({}),
            }],
        }];
        let result2 = AnthropicProtocol::convert_messages(&msgs2);
        let json2 = serde_json::to_value(&result2[0]).unwrap();
        let content2 = json2["content"].as_array().unwrap();
        let id2 = content2[0]["id"].as_str().unwrap();
        assert_eq!(id2.len(), 64);
    }

    #[test]
    fn test_convert_s10_image_content() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::User {
            content: vec![
                CB::Text {
                    text: "What is in this image?".to_string(),
                },
                CB::Image {
                    data: "aGVsbG8=".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        let json = serde_json::to_value(&result[0]).unwrap();
        // With multiple blocks, should be Multimodal (array content)
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is in this image?");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "aGVsbG8=");
    }
}
