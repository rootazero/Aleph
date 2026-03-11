//! OpenAI protocol adapter
//!
//! Handles OpenAI-compatible chat completion API format.
//! Used by: OpenAI, DeepSeek, Moonshot, Doubao, vLLM, etc.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::openai::{
    ChatCompletionResponse, ContentBlock, ImageUrl, Message, MessageContent, OpenAiFunction,
    OpenAiTool,
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

        // Add optional parameters (per-request overrides provider config)
        if let Some(max_tokens) = payload.max_tokens.or(config.max_tokens) {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(temp) = payload.temperature.or(config.temperature) {
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

        // Add tool definitions for function calling
        if let Some(tool_defs) = payload.tools {
            let tools: Vec<OpenAiTool> = tool_defs
                .iter()
                .map(|td| {
                    // Ensure parameters has "type" field — required by strict
                    // backends like AWS Bedrock, which rejects schemas without it.
                    let mut params = td.parameters.clone();
                    if let Some(obj) = params.as_object_mut() {
                        obj.entry("type").or_insert_with(|| json!("object"));
                    }
                    // Migrate schemars draft-07 schemas to draft 2020-12 for
                    // Bedrock and other strict backends.
                    crate::tools::schema_strictify::migrate_to_draft_2020_12(&mut params);
                    OpenAiTool {
                        tool_type: "function".into(),
                        function: OpenAiFunction {
                            name: td.name.clone(),
                            description: td.description.clone(),
                            parameters: params,
                            strict: if td.strict { Some(true) } else { None },
                        },
                    }
                })
                .collect();
            body["tools"] = serde_json::to_value(&tools).map_err(|e| {
                AlephError::provider(format!("Failed to serialize tools: {}", e))
            })?;
        }

        // Validate API key
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

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

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "OpenAI API error");
            return Err(AlephError::provider(format!(
                "API error ({}): {}",
                status, error_text
            )));
        }

        let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse OpenAI response");
            AlephError::provider(format!("Failed to parse response: {}", e))
        })?;

        let choice = completion
            .choices
            .first()
            .ok_or_else(|| AlephError::provider("No response choices"))?;

        let mut provider_response = ProviderResponse::default();

        // Extract text content (nullable when tool_calls present)
        if let Some(ref content) = choice.message.content {
            if !content.is_empty() {
                provider_response.text = Some(content.clone());
            }
        }

        // Extract tool calls
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let arguments: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_else(|e| {
                        error!(
                            tool = %tc.function.name,
                            args = %tc.function.arguments,
                            error = %e,
                            "Failed to parse tool call arguments, using empty object"
                        );
                        serde_json::Value::Object(Default::default())
                    });
                provider_response.tool_calls.push(NativeToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments,
                });
            }
        }

        // Map finish_reason to StopReason
        provider_response.stop_reason = match choice.finish_reason.as_deref() {
            Some("stop") => StopReason::EndTurn,
            Some("tool_calls") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            _ => StopReason::Unknown,
        };

        // Extract usage statistics
        if let Some(ref usage) = completion.usage {
            provider_response.usage = Some(TokenUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                cache_read_tokens: None,
            });
        }

        Ok(provider_response)
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "API error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .try_filter_map(|chunk| async move {
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

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

    fn supports_native_tools(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::providers::openai::{
        OpenAiFunctionCall, OpenAiToolCall,
    };

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

    // =========================================================================
    // Native Function Calling Tests
    // =========================================================================

    #[test]
    fn test_supports_native_tools() {
        let protocol = OpenAiProtocol::new(Client::new());
        assert!(protocol.supports_native_tools());
    }

    #[test]
    fn test_openai_tool_serialization() {
        let tool = OpenAiTool {
            tool_type: "function".into(),
            function: OpenAiFunction {
                name: "search".into(),
                description: "Search the web".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                }),
                strict: None,
            },
        };

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "search");
        assert_eq!(json["function"]["description"], "Search the web");
        assert!(json["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_parse_tool_calls_response() {
        // Simulate a real OpenAI response JSON with tool_calls
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"San Francisco\",\"unit\":\"celsius\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 20,
                "total_tokens": 70
            }
        });

        let response: ChatCompletionResponse =
            serde_json::from_value(response_json).unwrap();

        assert_eq!(response.choices.len(), 1);
        let choice = &response.choices[0];

        // content should be None (null in JSON)
        assert!(choice.message.content.is_none());

        // tool_calls should be present
        let tool_calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_abc123");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(
            tool_calls[0].function.arguments,
            r#"{"location":"San Francisco","unit":"celsius"}"#
        );

        // finish_reason should be tool_calls
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));

        // Usage should be present
        let usage = response.usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 50);
        assert_eq!(usage.completion_tokens, 20);
    }

    #[test]
    fn test_parse_text_response() {
        // Simulate a text-only response
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "Hello! How can I help you?"
                },
                "finish_reason": "stop"
            }]
        });

        let response: ChatCompletionResponse =
            serde_json::from_value(response_json).unwrap();

        let choice = &response.choices[0];
        assert_eq!(
            choice.message.content.as_deref(),
            Some("Hello! How can I help you?")
        );
        assert!(choice.message.tool_calls.is_none());
        assert_eq!(choice.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_function_arguments() {
        // Test that JSON string arguments parse correctly
        let tc = OpenAiToolCall {
            id: "call_123".into(),
            call_type: Some("function".into()),
            function: OpenAiFunctionCall {
                name: "search".into(),
                arguments: r#"{"query":"rust async","limit":10}"#.into(),
            },
        };

        let parsed: serde_json::Value =
            serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(parsed["query"], "rust async");
        assert_eq!(parsed["limit"], 10);
    }

    #[test]
    fn test_parse_malformed_function_arguments() {
        // Test fallback for malformed arguments
        let bad_args = "not valid json {{{";
        let result: serde_json::Value = serde_json::from_str(bad_args)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_build_request_includes_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::ToolCategory;

        let protocol = OpenAiProtocol::new(Client::new());
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
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config, false).unwrap();
        let built = request.build().unwrap();

        // Verify the body contains tools in OpenAI format
        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "search");
        assert_eq!(body["tools"][0]["function"]["description"], "Search the web");
        assert!(body["tools"][0]["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_build_request_no_tools_when_none() {
        let protocol = OpenAiProtocol::new(Client::new());
        let payload = RequestPayload::new("Hello");
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config, false).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        // tools field should be absent when no tools provided
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn test_build_request_with_multiple_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::ToolCategory;

        let protocol = OpenAiProtocol::new(Client::new());
        let tools = vec![
            ToolDefinition::new(
                "search",
                "Search the web",
                serde_json::json!({"type": "object", "properties": {}}),
                ToolCategory::Builtin,
            ),
            ToolDefinition::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                ToolCategory::Builtin,
            ),
        ];
        let payload = RequestPayload::new("Hello").with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config, false).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        let tools_array = body["tools"].as_array().unwrap();
        assert_eq!(tools_array.len(), 2);
        assert_eq!(tools_array[0]["function"]["name"], "search");
        assert_eq!(tools_array[1]["function"]["name"], "read_file");
    }
}
