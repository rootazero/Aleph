//! OpenAI protocol adapter
//!
//! Handles OpenAI-compatible chat completion API format.
//! Used by: OpenAI, DeepSeek, Moonshot, Doubao, vLLM, etc.

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::openai::{
    ChatCompletionResponse, ContentBlock, ImageUrl, Message, MessageContent,
};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
    should_use_prepend_mode,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use reqwest::Client;
use serde_json::json;
use tracing::{debug, error};

/// OpenAI protocol adapter
pub struct OpenAiProtocol {
    client: Client,
}

impl OpenAiProtocol {
    /// Create a new OpenAI protocol adapter with the given HTTP client
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL from provider configuration
    fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        // Detect API version from the URL (v1 or v3)
        let is_v3_api = raw_base_url.contains("/v3") || raw_base_url.contains("/api/v3");

        // Normalize URL: remove trailing slashes and version suffixes
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v3")
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        // Build endpoint with appropriate API version
        if is_v3_api {
            format!("{}/v3/chat/completions", base_url)
        } else {
            format!("{}/v1/chat/completions", base_url)
        }
    }

    /// Build messages array from request payload
    fn build_messages(payload: &RequestPayload, config: &ProviderConfig) -> Vec<Message> {
        let mut messages = Vec::new();
        let use_prepend_mode = !payload.force_standard_mode && should_use_prepend_mode(config);

        // Check if we have any image content
        let has_image = payload.image.is_some();
        let has_image_attachments = payload
            .attachments
            .map(|a| a.iter().any(|att| att.media_type == "image"))
            .unwrap_or(false);

        if has_image || has_image_attachments {
            Self::build_multimodal_messages(payload, config, use_prepend_mode, &mut messages);
        } else {
            Self::build_text_messages(payload, use_prepend_mode, &mut messages);
        }

        messages
    }

    /// Build text-only messages
    fn build_text_messages(
        payload: &RequestPayload,
        use_prepend_mode: bool,
        messages: &mut Vec<Message>,
    ) {
        // Handle document attachments by injecting into text
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

        if use_prepend_mode {
            // Prepend system prompt to user message (for APIs that ignore system role)
            let user_content = if let Some(prompt) = payload.system_prompt {
                format!(
                    "<<< SYSTEM INSTRUCTIONS - YOU MUST FOLLOW EXACTLY >>>\n\n{}\n\n<<< END INSTRUCTIONS >>>\n\n<<< USER INPUT >>>\n{}",
                    prompt, input
                )
            } else {
                input
            };

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: user_content,
                },
            });
        } else {
            // Standard mode: use separate system message
            if let Some(prompt) = payload.system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text { content: input },
            });
        }
    }

    /// Build multimodal messages with images
    fn build_multimodal_messages(
        payload: &RequestPayload,
        _config: &ProviderConfig,
        use_prepend_mode: bool,
        messages: &mut Vec<Message>,
    ) {
        // Add system message if not using prepend mode
        if !use_prepend_mode {
            if let Some(prompt) = payload.system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }
        }

        // Build content blocks
        let mut content_blocks = Vec::new();
        let mut text_input = payload.input.to_string();

        // Handle document attachments
        if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                text_input = combine_with_document_context(&doc_context, &text_input);
            }
        }

        // Build text content (with prepended system prompt if in prepend mode)
        let text_content = if use_prepend_mode {
            if let Some(prompt) = payload.system_prompt {
                format!("{}\n\n{}", prompt, text_input)
            } else {
                text_input
            }
        } else {
            text_input
        };

        // Provide default description for empty input
        let final_text = if text_content.is_empty() {
            "Describe this image in detail.".to_string()
        } else {
            text_content
        };

        content_blocks.push(ContentBlock::Text { text: final_text });

        // Add legacy image if present
        if let Some(image) = payload.image {
            content_blocks.push(ContentBlock::ImageUrl {
                image_url: ImageUrl {
                    url: image.to_base64(),
                    detail: Some("auto".to_string()),
                },
            });
        }

        // Add image attachments
        if let Some(attachments) = payload.attachments {
            let (images, _) = separate_attachments(attachments);
            for attachment in images {
                let data_uri = format!("data:{};base64,{}", attachment.mime_type, attachment.data);
                content_blocks.push(ContentBlock::ImageUrl {
                    image_url: ImageUrl {
                        url: data_uri,
                        detail: Some("auto".to_string()),
                    },
                });
            }
        }

        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        });
    }

    /// Map ThinkLevel to OpenAI reasoning_effort
    fn map_think_level(level: &crate::agents::thinking::ThinkLevel) -> Option<String> {
        use crate::agents::thinking::ThinkLevel;
        match level {
            ThinkLevel::Off | ThinkLevel::Minimal => None,
            ThinkLevel::Low => Some("low".to_string()),
            ThinkLevel::Medium => Some("medium".to_string()),
            ThinkLevel::High | ThinkLevel::XHigh => Some("high".to_string()),
        }
    }

    /// Parse a single SSE line and extract content
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            return None;
        }
        let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
        parsed["choices"][0]["delta"]["content"]
            .as_str()
            .map(|s| s.to_string())
    }
}

#[async_trait]
impl ProtocolAdapter for OpenAiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let messages = Self::build_messages(payload, config);

        // Build request body
        let mut body = json!({
            "model": &config.model,
            "messages": messages,
            "stream": is_streaming,
        });

        // Add optional parameters
        if let Some(max_tokens) = config.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(temp) = config.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(freq) = config.frequency_penalty {
            body["frequency_penalty"] = json!(freq);
        }
        if let Some(pres) = config.presence_penalty {
            body["presence_penalty"] = json!(pres);
        }

        // Add reasoning_effort for thinking models
        if let Some(ref level) = payload.think_level {
            if let Some(effort) = Self::map_think_level(level) {
                body["reasoning_effort"] = json!(effort);
            }
        }

        // Validate API key
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AetherError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building OpenAI request"
        );

        Ok(self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "OpenAI API error");
            return Err(AetherError::provider(format!(
                "API error ({}): {}",
                status, error_text
            )));
        }

        let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse OpenAI response");
            AetherError::provider(format!("Failed to parse response: {}", e))
        })?;

        completion
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| AetherError::provider("No response choices"))
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AetherError::provider(format!(
                "API error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map_err(|e| AetherError::network(format!("Stream error: {}", e)))
            .try_filter_map(|chunk| async move {
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| AetherError::provider(format!("UTF-8 error: {}", e)))?;

                let mut result = String::new();
                for line in text.lines() {
                    if let Some(content) = Self::parse_sse_line(line) {
                        result.push_str(&content);
                    }
                }

                if result.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(result))
                }
            });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("deepseek-chat");
        config.base_url = Some("https://api.deepseek.com".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.deepseek.com/v1/chat/completions");
    }

    #[test]
    fn test_build_endpoint_v3() {
        let mut config = ProviderConfig::test_config("doubao-pro");
        config.base_url = Some("https://ark.cn-beijing.volces.com/api/v3".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(
            endpoint,
            "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
        );
    }

    #[test]
    fn test_build_endpoint_with_trailing_slash() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://api.example.com/v1/".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.example.com/v1/chat/completions");
    }

    #[test]
    fn test_map_think_level() {
        use crate::agents::thinking::ThinkLevel;

        assert!(OpenAiProtocol::map_think_level(&ThinkLevel::Off).is_none());
        assert!(OpenAiProtocol::map_think_level(&ThinkLevel::Minimal).is_none());
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::Low),
            Some("low".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::Medium),
            Some("medium".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::High),
            Some("high".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::XHigh),
            Some("high".to_string())
        );
    }

    #[test]
    fn test_parse_sse_line() {
        // Valid SSE line
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(OpenAiProtocol::parse_sse_line(line), Some("Hello".to_string()));

        // Done marker
        let done = "data: [DONE]";
        assert!(OpenAiProtocol::parse_sse_line(done).is_none());

        // Empty line
        assert!(OpenAiProtocol::parse_sse_line("").is_none());

        // Non-data line
        assert!(OpenAiProtocol::parse_sse_line("event: message").is_none());
    }

    #[test]
    fn test_build_messages_text_only() {
        let config = ProviderConfig::test_config("gpt-4o");
        let payload = RequestPayload::new("Hello, world!");

        let messages = OpenAiProtocol::build_messages(&payload, &config);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    #[test]
    fn test_build_messages_with_system_prompt_standard_mode() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.system_prompt_mode = Some("standard".to_string());

        let payload = RequestPayload::new("Hello")
            .with_system(Some("You are helpful"));

        let messages = OpenAiProtocol::build_messages(&payload, &config);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
    }

    #[test]
    fn test_build_messages_with_system_prompt_prepend_mode() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.system_prompt_mode = Some("prepend".to_string());

        let payload = RequestPayload::new("Hello")
            .with_system(Some("You are helpful"));

        let messages = OpenAiProtocol::build_messages(&payload, &config);

        // In prepend mode, system prompt is merged into user message
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }
}
