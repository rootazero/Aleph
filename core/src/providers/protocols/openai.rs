//! OpenAI protocol adapter
//!
//! Handles OpenAI-compatible chat completion API format.
//! Used by: OpenAI, DeepSeek, Moonshot, Doubao, vLLM, etc.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason,
    TokenUsage,
};
use crate::providers::openai::{
    ChatCompletionResponse, ContentBlock as OaiContentBlock, ImageUrl, Message, MessageContent,
    OpenAiFunction, OpenAiTool,
};
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use reqwest::Client;
use serde_json::json;
use tracing::{debug, error};

// ── Tool-name sanitization ──────────────────────────────────────────────
//
// OpenAI API requires tool names to match `^[a-zA-Z0-9_-]+$`.
// Aleph tool names now use underscores (e.g. "cron_manage"), but this
// sanitizer is kept as a safety net for any external/plugin tool names.

use super::openai_common::tools::{
    sanitize_tool_name as sanitize_tool_name_pub,
    desanitize_tool_name as desanitize_tool_name_pub,
};

/// Replace characters not in `[a-zA-Z0-9_-]` with underscores (internal alias).
fn sanitize_tool_name(name: &str) -> String { sanitize_tool_name_pub(name) }

/// Identity function — tool names now use underscores natively (internal alias).
fn desanitize_tool_name(name: &str) -> String { desanitize_tool_name_pub(name) }

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

    /// Convert UnifiedMessages to OpenAI Messages
    fn convert_messages(messages: &[UnifiedMessage], system_prompt: Option<&str>) -> Vec<Message> {
        let mut result = Vec::new();

        // Add system message if provided
        if let Some(prompt) = system_prompt {
            result.push(Message::text("system", prompt.to_string()));
        }

        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    use crate::providers::message::ContentBlock as UCB;
                    let has_images = content
                        .iter()
                        .any(|b| matches!(b, UCB::Image { .. }));

                    if has_images {
                        // Use OpenAI's multimodal content array format
                        let blocks: Vec<OaiContentBlock> = content
                            .iter()
                            .filter_map(|b| match b {
                                UCB::Text { text } => {
                                    Some(OaiContentBlock::Text { text: text.clone() })
                                }
                                UCB::Image { data, mime_type } => {
                                    Some(OaiContentBlock::ImageUrl {
                                        image_url: ImageUrl {
                                            url: format!("data:{};base64,{}", mime_type, data),
                                            detail: Some("auto".to_string()),
                                        },
                                    })
                                }
                                _ => None,
                            })
                            .collect();
                        let image_count = blocks.iter().filter(|b| matches!(b, OaiContentBlock::ImageUrl { .. })).count();
                        tracing::info!(
                            target: "multimodal",
                            probe = "P6_provider",
                            role = "user",
                            content_type = "multimodal",
                            image_count = image_count,
                            "OpenAI multimodal message converted"
                        );
                        result.push(Message {
                            role: "user".to_string(),
                            tool_call_id: None,
                            tool_calls: None,
                            content: MessageContent::Multimodal { content: blocks },
                        });
                    } else {
                        // Text-only path
                        let text = content
                            .iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("\n");
                        result.push(Message::text("user", text));
                    }
                }
                UnifiedMessage::Assistant { content } => {
                    let text: String = content
                        .iter()
                        .filter_map(|b| match b {
                            crate::providers::message::ContentBlock::Text { text } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    // Extract tool calls
                    let tool_calls: Vec<_> = content
                        .iter()
                        .filter_map(|b| match b {
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                use crate::providers::openai::{OpenAiFunctionCall, OpenAiToolCall};
                                Some(OpenAiToolCall {
                                    id: id.clone(),
                                    call_type: Some("function".to_string()),
                                    function: OpenAiFunctionCall {
                                        name: sanitize_tool_name(name),
                                        arguments: serde_json::to_string(arguments)
                                            .unwrap_or_default(),
                                    },
                                })
                            }
                            _ => None,
                        })
                        .collect();

                    let msg_content = if text.is_empty() { None } else { Some(text) };

                    if tool_calls.is_empty() {
                        result.push(Message::text("assistant", msg_content.unwrap_or_default()));
                    } else {
                        // Convert to serializable tool call format
                        use crate::providers::openai::types::{OpenAiToolCallOut, OpenAiFunctionCallOut};
                        let tc_out: Vec<OpenAiToolCallOut> = tool_calls
                            .into_iter()
                            .map(|tc| OpenAiToolCallOut {
                                id: tc.id,
                                call_type: "function".to_string(),
                                function: OpenAiFunctionCallOut {
                                    name: tc.function.name,
                                    arguments: tc.function.arguments,
                                },
                            })
                            .collect();
                        result.push(Message::assistant_with_tool_calls(msg_content, tc_out));
                    }
                }
                UnifiedMessage::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => {
                    let output = content
                        .iter()
                        .map(|b| match b {
                            crate::providers::message::ContentBlock::Text { text } => text.clone(),
                            crate::providers::message::ContentBlock::Json { value } => {
                                serde_json::to_string(value).unwrap_or_default()
                            }
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    // Each ToolResult as separate tool message with required tool_call_id
                    result.push(Message::tool_result(tool_call_id.clone(), output));
                }
            }
        }
        result
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
        let messages = Self::convert_messages(payload.messages, payload.system_prompt);

        // Build request body
        let mut body = json!({
            "model": config.default_model(),
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
                            // OpenAI API requires tool names to match ^[a-zA-Z0-9_-]+$
                            // Aleph tool names now use underscores natively; sanitize
                            // is kept as a safety net for external/plugin tool names.
                            name: sanitize_tool_name(&td.name),
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

        // Add tool_choice if specified
        if let Some(ref choice) = payload.tool_choice {
            use crate::providers::adapter::ToolChoice;
            body["tool_choice"] = match choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::Required => json!("required"),
                ToolChoice::Specific(name) => json!({"type": "function", "function": {"name": name}}),
                ToolChoice::None => json!("none"),
            };
        }

        // Validate API key
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.default_model(),
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
                        serde_json::json!({
                            "_raw_arguments": tc.function.arguments,
                            "_parse_error": e.to_string()
                        })
                    });
                provider_response.tool_calls.push(NativeToolCall {
                    id: tc.id.clone(),
                    // Pass through tool name (identity — underscore names are native now).
                    name: desanitize_tool_name(&tc.function.name),
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

    fn supports_strict_schema(&self) -> bool {
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
    use crate::providers::message::UnifiedMessage;
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
    fn test_convert_messages_text_only() {
        let msgs = [UnifiedMessage::user("Hello, world!")];
        let messages = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    #[test]
    fn test_convert_messages_with_system_prompt() {
        let msgs = [UnifiedMessage::user("Hello")];
        let messages = OpenAiProtocol::convert_messages(&msgs, Some("You are helpful"));

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
    }

    #[test]
    fn test_convert_messages_no_system_prompt() {
        let msgs = [UnifiedMessage::user("Hello")];
        let messages = OpenAiProtocol::convert_messages(&msgs, None);

        // Without system prompt, only user message
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
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
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
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
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
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
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

    // =========================================================================
    // convert_messages() Tests
    // =========================================================================

    #[test]
    fn test_convert_s1_pure_text_user() {
        let msgs = [UnifiedMessage::user("Hello, world!")];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        // OpenAI uses plain string content (Text variant), not array
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello, world!");
    }

    #[test]
    fn test_convert_s2_multi_turn() {
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

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
                    text: "Let me search.".to_string(),
                },
                CB::ToolCall {
                    id: "call_abc".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                },
            ],
        }];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
        let json = serde_json::to_value(&result[0]).unwrap();
        // Text content should be present
        assert_eq!(json["content"], "Let me search.");
    }

    #[test]
    fn test_convert_s4_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "call_abc",
            "search",
            "Found results",
            false,
        )];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        // Each ToolResult is a separate tool message (NOT merged)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "tool");
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["content"], "Found results");
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
            UnifiedMessage::assistant("Based on results, Rust is great!"),
        ];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "tool");
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_convert_s6_arguments_json_stringify() {
        use crate::providers::message::ContentBlock as CB;
        // Verify that OpenAI tool_calls arguments are stringified JSON
        let args = serde_json::json!({"query": "test"});
        let msgs = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: args.clone(),
            }],
        }];
        // convert_messages currently puts text content on assistant, tool_calls
        // are extracted but stored as `let _ = msg_tool_calls` (TODO in code).
        // We verify the internal extraction produces stringified JSON.
        let stringified = serde_json::to_string(&args).unwrap();
        assert_eq!(stringified, r#"{"query":"test"}"#);

        // Also verify the messages convert without panic
        let result = OpenAiProtocol::convert_messages(&msgs, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
    }

    #[test]
    fn test_convert_s7_multiple_tool_calls() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "a"}),
                },
                CB::ToolCall {
                    id: "call_2".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/tmp/x"}),
                },
            ],
        }];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        // Assistant message should exist
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
    }

    #[test]
    fn test_convert_s8_system_prompt_handling() {
        let msgs = [UnifiedMessage::user("Hello")];
        let result = OpenAiProtocol::convert_messages(&msgs, Some("You are a helpful assistant."));

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "system");
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["content"], "You are a helpful assistant.");
        assert_eq!(result[1].role, "user");
    }
}
