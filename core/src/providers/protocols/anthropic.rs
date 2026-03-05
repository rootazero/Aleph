//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::anthropic::{
    AnthropicContentBlock, AnthropicTool, ContentBlock, ErrorResponse, ImageSource, Message,
    MessageContent, MessagesRequest, MessagesResponse, SystemBlock, ThinkingBlock,
};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
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

    /// Build messages from payload
    fn build_messages(payload: &RequestPayload, _config: &ProviderConfig) -> Vec<Message> {
        // Check for image content
        let has_image = payload.image.is_some();
        let has_image_attachments = payload
            .attachments
            .map(|a| a.iter().any(|att| att.media_type == "image"))
            .unwrap_or(false);

        if has_image || has_image_attachments {
            Self::build_multimodal_messages(payload)
        } else {
            Self::build_text_messages(payload)
        }
    }

    /// Build text-only messages
    fn build_text_messages(payload: &RequestPayload) -> Vec<Message> {
        let input = if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                combine_with_document_context(&doc_context, payload.input)
            } else {
                payload.input.to_string()
            }
        } else {
            payload.input.to_string()
        };

        vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text { content: input },
        }]
    }

    /// Build multimodal messages with images
    fn build_multimodal_messages(payload: &RequestPayload) -> Vec<Message> {
        let mut content_blocks = Vec::new();

        // Handle document attachments
        let text_input = if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                combine_with_document_context(&doc_context, payload.input)
            } else {
                payload.input.to_string()
            }
        } else {
            payload.input.to_string()
        };

        // Add text content
        let text = if text_input.is_empty() {
            "Describe this image in detail.".to_string()
        } else {
            text_input
        };
        content_blocks.push(ContentBlock::Text { text });

        // Add legacy image
        if let Some(image) = payload.image {
            let media_type = match image.format {
                crate::clipboard::ImageFormat::Png => "image/png",
                crate::clipboard::ImageFormat::Jpeg => "image/jpeg",
                crate::clipboard::ImageFormat::Gif => "image/gif",
            };
            let base64_data = {
                use base64::{engine::general_purpose, Engine as _};
                general_purpose::STANDARD.encode(&image.data)
            };
            content_blocks.push(ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: media_type.to_string(),
                    data: base64_data,
                },
            });
        }

        // Add image attachments
        if let Some(attachments) = payload.attachments {
            let (images, _) = separate_attachments(attachments);
            for attachment in images {
                content_blocks.push(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: attachment.mime_type.clone(),
                        data: attachment.data.clone(),
                    },
                });
            }
        }

        vec![Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        }]
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
        let messages = Self::build_messages(payload, config);

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
                    AnthropicTool {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        input_schema: schema,
                    }
                })
                .collect()
        });

        let request_body = MessagesRequest {
            model: config.model.clone(),
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
            model = %config.model,
            streaming = is_streaming,
            "Building Anthropic request"
        );

        Ok(self
            .client
            .post(&endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .json(&request_body))
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
        let payload = RequestPayload::new("Hello").with_tools(Some(&tools));
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
        let protocol = AnthropicProtocol::new(Client::new());
        let payload = RequestPayload::new("Hello");
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config, false).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        // tools field should be absent (skip_serializing_if = "Option::is_none")
        assert!(body.get("tools").is_none());
    }
}
