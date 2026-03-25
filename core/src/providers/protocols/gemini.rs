//! Google Gemini protocol adapter
//!
//! Handles Google Generative AI API format.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    ProtocolAdapter, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::delta::ProviderDelta;
use crate::providers::gemini::{
    Content, GeminiFunctionDeclaration, GeminiToolConfig, GenerateContentRequest,
    GenerationConfig, Part, ThinkingConfig,
};
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use std::collections::VecDeque;
use tracing::debug;

/// Google Gemini protocol adapter
pub struct GeminiProtocol {
    client: Client,
}

impl GeminiProtocol {
    /// Create a new Gemini protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL — always uses the streaming endpoint (stream-first architecture)
    fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        // Normalize URL: strip trailing slashes and /v1 suffix
        // (user may have /v1 from switching between OpenAI/Anthropic protocols)
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        // Always use the streaming endpoint
        format!(
            "{}/v1beta/models/{}:streamGenerateContent",
            base_url, config.default_model()
        )
    }

    /// Convert UnifiedMessages to Gemini Contents
    fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Content> {
        let mut result = Vec::new();
        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    let text = content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts: vec![Part::Text { text }],
                    });
                }
                UnifiedMessage::Assistant { content } => {
                    let mut parts = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text } => {
                                parts.push(Part::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::ToolCall {
                                name,
                                arguments,
                                ..
                            } => {
                                parts.push(Part::FunctionCall {
                                    function_call: crate::providers::gemini::GeminiFunctionCall {
                                        name: name.clone(),
                                        args: arguments.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    if parts.is_empty() {
                        parts.push(Part::Text {
                            text: String::new(),
                        });
                    }
                    result.push(Content {
                        role: Some("model".to_string()),
                        parts,
                    });
                }
                UnifiedMessage::ToolResult {
                    tool_name,
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
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts: vec![Part::FunctionResponse {
                            function_response: crate::providers::gemini::GeminiFunctionResponse {
                                name: tool_name.clone(),
                                response: serde_json::json!({ "result": output }),
                            },
                        }],
                    });
                }
            }
        }
        result
    }

    /// Build system instruction from system prompt
    fn build_system_instruction(system_prompt: Option<&str>) -> Option<Content> {
        system_prompt.map(|prompt| Content {
            role: None, // system instruction doesn't have a role
            parts: vec![Part::Text {
                text: prompt.to_string(),
            }],
        })
    }

    /// Map ThinkLevel to thinkingBudget
    fn map_think_level(level: &ThinkLevel) -> Option<u32> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(500),
            ThinkLevel::Low => Some(1000),
            ThinkLevel::Medium => Some(2000),
            ThinkLevel::High => Some(4000),
            ThinkLevel::XHigh => Some(8000),
        }
    }

}

#[async_trait]
impl ProtocolAdapter for GeminiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let contents = Self::convert_messages(payload.messages);
        let system_instruction = Self::build_system_instruction(payload.system_prompt);

        // Build generation config
        let thinking_config = payload
            .think_level
            .as_ref()
            .and_then(Self::map_think_level)
            .map(|budget| ThinkingConfig {
                thinking_budget: Some(budget),
            });

        // Per-request overrides provider config
        let generation_config = GenerationConfig {
            max_output_tokens: payload.max_tokens.or(config.max_tokens).or(Some(DEFAULT_MAX_TOKENS)),
            temperature: payload.temperature.or(config.temperature),
            top_p: config.top_p,
            top_k: None,
            thinking_config,
        };

        // Build tool declarations if provided
        let tools = payload.tools.map(|tool_defs| {
            let declarations: Vec<GeminiFunctionDeclaration> = tool_defs
                .iter()
                .map(|td| {
                    // Ensure parameters has "type" field — required by strict
                    // backends; keeps parity with Anthropic/OpenAI adapters.
                    let mut params = td.parameters.clone();
                    if let Some(obj) = params.as_object_mut() {
                        obj.entry("type")
                            .or_insert_with(|| serde_json::json!("object"));
                    }
                    GeminiFunctionDeclaration {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: params,
                    }
                })
                .collect();
            vec![GeminiToolConfig {
                function_declarations: declarations,
            }]
        });

        let request_body = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config: Some(generation_config),
            tools,
        };

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.default_model(),
            "Building Gemini request"
        );

        // Build URL with query parameters
        let mut url = endpoint;
        url.push_str("?key=");
        url.push_str(api_key);
        // Always add alt=sse for streaming (stream-first architecture)
        url.push_str("&alt=sse");

        // Serialize to JSON value so we can add tool_config if needed
        let mut body = serde_json::to_value(&request_body)
            .map_err(|e| AlephError::provider(format!("Failed to serialize request: {}", e)))?;

        // Add tool_config if tool_choice is specified
        if let Some(ref choice) = payload.tool_choice {
            use crate::providers::adapter::ToolChoice;
            body["tool_config"] = match choice {
                ToolChoice::Auto => serde_json::json!({"function_calling_config": {"mode": "AUTO"}}),
                ToolChoice::Required => serde_json::json!({"function_calling_config": {"mode": "ANY"}}),
                ToolChoice::Specific(name) => serde_json::json!({"function_calling_config": {
                    "mode": "ANY", "allowed_function_names": [name]
                }}),
                ToolChoice::None => serde_json::json!({"function_calling_config": {"mode": "NONE"}}),
            };
        }

        Ok(self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body))
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    /// Stream fine-grained [`ProviderDelta`] events from a Gemini SSE response.
    ///
    /// Gemini streams complete function calls per chunk (not incremental args), so
    /// each `functionCall` part yields `ToolCallStart + ToolCallArgDelta + ToolCallEnd`
    /// in one shot. Synthetic call IDs are generated as `gemini_fc_{counter}` because
    /// the Gemini API does not assign tool call IDs.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Gemini streaming error ({}): {}",
                status, error_text
            )));
        }

        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .boxed();

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE line buffer (handles chunk boundaries)
            line_buf: String,
            /// Monotonically increasing counter for synthetic tool call IDs
            fc_counter: u64,
            /// Pending deltas queued from multi-delta events
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: String::new(),
            fc_counter: 0,
            pending: VecDeque::new(),
            done: false,
        };

        let stream = futures::stream::unfold(state, |mut state| async move {
            loop {
                // Drain pending queue first
                if let Some(delta) = state.pending.pop_front() {
                    return Some((delta, state));
                }

                if state.done {
                    return None;
                }

                // Try to parse a complete SSE line from the buffer
                if let Some(pos) = state.line_buf.find('\n') {
                    let line = state.line_buf[..pos].trim_end().to_string();
                    state.line_buf.drain(..=pos);

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data != "[DONE]" {
                            parse_gemini_sse_chunk(
                                data,
                                &mut state.fc_counter,
                                &mut state.pending,
                            );
                            // If Done was queued, stop after draining pending
                            if state.pending.iter().any(|d| {
                                matches!(d, Ok(ProviderDelta::Done(_)))
                            }) {
                                state.done = true;
                            }
                        }
                    }
                    continue;
                }

                // No complete line — fetch next chunk from HTTP
                match state.bytes.next().await {
                    None => {
                        // HTTP stream ended — flush any remaining partial line
                        let remaining = state.line_buf.trim().to_string();
                        state.line_buf.clear();
                        if !remaining.is_empty() {
                            if let Some(data) = remaining.strip_prefix("data: ") {
                                if data != "[DONE]" {
                                    parse_gemini_sse_chunk(
                                        data,
                                        &mut state.fc_counter,
                                        &mut state.pending,
                                    );
                                }
                            }
                        }
                        state.done = true;
                        if let Some(delta) = state.pending.pop_front() {
                            return Some((delta, state));
                        }
                        return None;
                    }
                    Some(Err(e)) => {
                        state.done = true;
                        return Some((Err(e), state));
                    }
                    Some(Ok(chunk)) => match std::str::from_utf8(&chunk) {
                        Ok(text) => state.line_buf.push_str(text),
                        Err(e) => {
                            state.done = true;
                            return Some((
                                Err(AlephError::provider(format!("UTF-8 error: {}", e))),
                                state,
                            ));
                        }
                    },
                }
            }
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "gemini"
    }
}

// =============================================================================
// SSE parsing helper for Gemini streaming format
// =============================================================================

/// Parse one Gemini SSE data JSON chunk and push [`ProviderDelta`] events into `out`.
///
/// Gemini SSE chunk format:
/// ```json
/// {"candidates":[{"content":{"parts":[{"text":"Hello"}]},"finishReason":"STOP"}],
///  "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}
/// ```
///
/// Since Gemini delivers **complete** function calls (not incremental arg fragments),
/// each `functionCall` part immediately emits `ToolCallStart + ToolCallArgDelta + ToolCallEnd`.
/// Synthetic IDs are generated as `gemini_fc_{counter}` (Gemini has no real IDs).
fn parse_gemini_sse_chunk(
    data: &str,
    fc_counter: &mut u64,
    out: &mut VecDeque<Result<ProviderDelta>>,
) {
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            out.push_back(Err(AlephError::provider(format!(
                "Gemini SSE parse error: {}",
                e
            ))));
            return;
        }
    };

    // Extract candidate[0]
    let candidate = json
        .get("candidates")
        .and_then(|c| c.get(0));

    if let Some(candidate) = candidate {
        // Process content parts
        if let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
        {
            for part in parts {
                // Text delta
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        out.push_back(Ok(ProviderDelta::TextDelta(text.to_string())));
                    }
                }

                // Function call — complete in one chunk, emit Start+ArgDelta+End
                if let Some(fc) = part.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(serde_json::Value::Null);
                    let args_str = args.to_string();

                    // Generate synthetic ID (Gemini provides no call IDs)
                    let id = format!("gemini_fc_{}", *fc_counter);
                    *fc_counter += 1;

                    out.push_back(Ok(ProviderDelta::ToolCallStart {
                        id: id.clone(),
                        name,
                    }));
                    if !args_str.is_empty() && args_str != "null" {
                        out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                            id: id.clone(),
                            delta: args_str,
                        }));
                    }
                    out.push_back(Ok(ProviderDelta::ToolCallEnd { id }));
                }
            }
        }

        // Map finishReason to Done
        let finish_reason = candidate
            .get("finishReason")
            .and_then(|r| r.as_str());

        let has_tool_calls = out.iter().any(|d| {
            matches!(d, Ok(ProviderDelta::ToolCallStart { .. }))
        });

        let stop_reason = match finish_reason {
            Some("STOP") => Some(StopReason::EndTurn),
            Some("MAX_TOKENS") => Some(StopReason::MaxTokens),
            Some("FUNCTION_CALL") => Some(StopReason::ToolUse),
            Some(other) if !other.is_empty() => {
                // If we emitted tool calls in this same chunk, treat as ToolUse
                if has_tool_calls {
                    Some(StopReason::ToolUse)
                } else {
                    Some(StopReason::Unknown)
                }
            }
            _ => {
                // No finish reason in this chunk — check if we saw tool calls
                // without an explicit reason (some Gemini variants omit the field)
                None
            }
        };

        if let Some(reason) = stop_reason {
            out.push_back(Ok(ProviderDelta::Done(reason)));
        }
    }

    // Usage metadata (usually in the last chunk)
    if let Some(usage) = json.get("usageMetadata") {
        let input = usage
            .get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let output = usage
            .get("candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        // Insert Usage before the Done event so consumers see it in the right order
        let done_pos = out.iter().position(|d| matches!(d, Ok(ProviderDelta::Done(_))));
        let usage_event = Ok(ProviderDelta::Usage(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: None,
            thinking_tokens: None,
        }));

        if let Some(pos) = done_pos {
            // Splice Usage before Done
            out.insert(pos, usage_event);
        } else {
            out.push_back(usage_event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse};
    use crate::providers::gemini::GenerateContentResponse;
    use crate::providers::message::UnifiedMessage;

    #[test]
    fn test_build_endpoint_always_streaming() {
        // Stream-first architecture: always uses streamGenerateContent endpoint
        let config = ProviderConfig::test_config("gemini-pro");
        let endpoint = GeminiProtocol::build_endpoint(&config);
        assert_eq!(
            endpoint,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:streamGenerateContent"
        );
    }

    #[test]
    fn test_build_endpoint_custom_base_url() {
        let mut config = ProviderConfig::test_config("gemini-pro");
        config.base_url = Some("https://custom.api.com".to_string());
        let endpoint = GeminiProtocol::build_endpoint(&config);
        assert_eq!(
            endpoint,
            "https://custom.api.com/v1beta/models/gemini-pro:streamGenerateContent"
        );
    }

    #[test]
    fn test_map_think_level() {
        assert_eq!(GeminiProtocol::map_think_level(&ThinkLevel::Off), None);
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::Minimal),
            Some(500)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::Low),
            Some(1000)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::Medium),
            Some(2000)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::High),
            Some(4000)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::XHigh),
            Some(8000)
        );
    }

    #[test]
    fn test_convert_messages_text() {
        let msgs = [UnifiedMessage::user("Hello, Gemini!")];
        let contents = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, Some("user".to_string()));
        assert_eq!(contents[0].parts.len(), 1);

        if let Part::Text { text } = &contents[0].parts[0] {
            assert_eq!(text, "Hello, Gemini!");
        } else {
            panic!("Expected text part");
        }
    }

    #[test]
    fn test_build_system_instruction() {
        let instruction = GeminiProtocol::build_system_instruction(Some("You are helpful"));

        assert!(instruction.is_some());
        let content = instruction.unwrap();
        assert_eq!(content.role, None);
        assert_eq!(content.parts.len(), 1);

        if let Part::Text { text } = &content.parts[0] {
            assert_eq!(text, "You are helpful");
        } else {
            panic!("Expected text part");
        }
    }

    #[test]
    fn test_parse_response_error() {
        // Test error parsing logic
        let error_json = r#"{
            "error": {
                "code": 400,
                "message": "Invalid request",
                "status": "INVALID_ARGUMENT"
            }
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(error_json).unwrap();
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, 400);
    }

    #[test]
    fn test_parse_response_success() {
        // Test successful response parsing
        let success_json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "This is a test response"}]
                },
                "finishReason": "STOP"
            }]
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(success_json).unwrap();
        assert!(response.candidates.is_some());

        let candidates = response.candidates.unwrap();
        let text = candidates[0].content.parts[0].text.as_deref();
        assert_eq!(text, Some("This is a test response"));
        assert_eq!(candidates[0].finish_reason.as_deref(), Some("STOP"));
    }

    #[test]
    fn test_build_request_basic() {
        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);

        let request = protocol
            .build_request(&payload, &config)
            .expect("Failed to build request");

        // Always uses streaming endpoint (stream-first architecture)
        let url = request.build().unwrap().url().to_string();
        assert!(url.contains("streamGenerateContent"));
        assert!(url.contains("key=test-api-key"));
    }

    #[test]
    fn test_build_request_with_thinking() {
        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let msgs = [UnifiedMessage::user("Solve this problem")];
        let payload = RequestPayload::new(&msgs)
            .with_think_level(Some(ThinkLevel::Medium));

        let request = protocol
            .build_request(&payload, &config)
            .expect("Failed to build request");

        // We can't easily inspect the request body, but we can verify it builds successfully
        assert!(request.build().is_ok());
    }

    #[test]
    fn test_supports_native_tools() {
        let protocol = GeminiProtocol::new(Client::new());
        assert!(protocol.supports_native_tools());
    }

    #[test]
    fn test_build_request_with_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::ToolCategory;

        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let tools = vec![ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
            ToolCategory::Builtin,
        )];

        let msgs = [UnifiedMessage::user("Search for Rust")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));

        let request = protocol
            .build_request(&payload, &config)
            .expect("Failed to build request");

        assert!(request.build().is_ok());
    }

    /// Helper: simulate parse_response logic on a deserialized GenerateContentResponse
    /// (avoids needing to construct a real reqwest::Response in unit tests)
    fn extract_provider_response(
        response_body: GenerateContentResponse,
    ) -> Result<ProviderResponse> {
        if let Some(err) = response_body.error {
            return Err(AlephError::provider(format!(
                "Gemini error: {}",
                err.message
            )));
        }

        let candidates = response_body
            .candidates
            .ok_or_else(|| AlephError::provider("No candidates in response"))?;
        let candidate = candidates
            .first()
            .ok_or_else(|| AlephError::provider("No candidates in response"))?;

        let mut provider_response = ProviderResponse::default();

        let mut text_parts = Vec::new();
        for (index, part) in candidate.content.parts.iter().enumerate() {
            if let Some(ref text) = part.text {
                text_parts.push(text.clone());
            }
            if let Some(ref fc) = part.function_call {
                provider_response.tool_calls.push(NativeToolCall {
                    id: format!("gemini-fc-{}", index),
                    name: fc.name.clone(),
                    arguments: fc.args.clone(),
                });
            }
        }

        if !text_parts.is_empty() {
            provider_response.text = Some(text_parts.join(""));
        }

        provider_response.stop_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => StopReason::EndTurn,
            Some("FUNCTION_CALL") => StopReason::ToolUse,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            _ => StopReason::Unknown,
        };

        Ok(provider_response)
    }

    #[test]
    fn test_extract_response_text_only() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello world"}]
                },
                "finishReason": "STOP"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Hello world"));
        assert!(!result.has_tool_calls());
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_extract_response_with_function_call() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "search",
                            "args": {"query": "Rust programming"}
                        }
                    }]
                },
                "finishReason": "FUNCTION_CALL"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert!(result.text.is_none());
        assert!(result.has_tool_calls());
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "search");
        assert_eq!(result.tool_calls[0].arguments["query"], "Rust programming");
        assert!(result.tool_calls[0].id.starts_with("gemini-"));
        assert_eq!(result.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_extract_response_with_text_and_function_call() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Let me search for that."},
                        {"functionCall": {"name": "web_search", "args": {"q": "test"}}}
                    ]
                },
                "finishReason": "FUNCTION_CALL"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Let me search for that."));
        assert!(result.has_tool_calls());
        assert_eq!(result.tool_calls[0].name, "web_search");
        assert_eq!(result.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_extract_response_max_tokens() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Truncated output..."}]
                },
                "finishReason": "MAX_TOKENS"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Truncated output..."));
        assert_eq!(result.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn test_extract_response_unknown_finish_reason() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Some text"}]
                },
                "finishReason": "SAFETY"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.stop_reason, StopReason::Unknown);
    }

    #[test]
    fn test_extract_response_no_finish_reason() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Partial response"}]
                }
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Partial response"));
        assert_eq!(result.stop_reason, StopReason::Unknown);
    }

    // =========================================================================
    // convert_messages() Tests
    // =========================================================================

    #[test]
    fn test_convert_s1_pure_text_user() {
        let msgs = [UnifiedMessage::user("Hello, Gemini!")];
        let result = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Some("user".to_string()));
        assert_eq!(result[0].parts.len(), 1);
        match &result[0].parts[0] {
            Part::Text { text } => assert_eq!(text, "Hello, Gemini!"),
            _ => panic!("Expected Text part"),
        }
    }

    #[test]
    fn test_convert_s2_multi_turn() {
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let result = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, Some("user".to_string()));
        assert_eq!(result[1].role, Some("model".to_string()));
        assert_eq!(result[2].role, Some("user".to_string()));
    }

    #[test]
    fn test_convert_s3_assistant_with_tool_call() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "rust"}),
            }],
        }];
        let result = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Some("model".to_string()));
        // Serialize to check JSON structure
        let json = serde_json::to_value(&result[0]).unwrap();
        let parts = json["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert!(parts[0].get("functionCall").is_some());
        assert_eq!(parts[0]["functionCall"]["name"], "search");
        assert_eq!(parts[0]["functionCall"]["args"]["query"], "rust");
    }

    #[test]
    fn test_convert_s4_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "call_1",
            "search",
            "Found 3 results",
            false,
        )];
        let result = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Some("user".to_string()));
        let json = serde_json::to_value(&result[0]).unwrap();
        let parts = json["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert!(parts[0].get("functionResponse").is_some());
        assert_eq!(parts[0]["functionResponse"]["name"], "search");
        assert_eq!(
            parts[0]["functionResponse"]["response"]["result"],
            "Found 3 results"
        );
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
        let result = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, Some("user".to_string()));
        assert_eq!(result[1].role, Some("model".to_string()));
        assert_eq!(result[2].role, Some("user".to_string())); // tool result as user
        assert_eq!(result[3].role, Some("model".to_string()));
    }

    #[test]
    fn test_convert_s6_consecutive_tool_results_separate() {
        // Gemini does NOT merge consecutive ToolResults (unlike Anthropic);
        // each becomes a separate user Content entry
        let msgs = [
            UnifiedMessage::tool_result("call_1", "search", "result 1", false),
            UnifiedMessage::tool_result("call_2", "read_file", "result 2", false),
        ];
        let result = GeminiProtocol::convert_messages(&msgs);

        // Each ToolResult becomes its own Content
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Some("user".to_string()));
        assert_eq!(result[1].role, Some("user".to_string()));
        // Both should have functionResponse parts
        let json0 = serde_json::to_value(&result[0]).unwrap();
        let json1 = serde_json::to_value(&result[1]).unwrap();
        assert!(json0["parts"][0].get("functionResponse").is_some());
        assert!(json1["parts"][0].get("functionResponse").is_some());
    }

    #[test]
    fn test_convert_s7_role_assignment() {
        // Verify user/model roles are correctly assigned
        let msgs = [
            UnifiedMessage::user("msg1"),
            UnifiedMessage::assistant("msg2"),
            UnifiedMessage::user("msg3"),
            UnifiedMessage::assistant("msg4"),
        ];
        let result = GeminiProtocol::convert_messages(&msgs);

        let roles: Vec<&str> = result
            .iter()
            .map(|c| c.role.as_deref().unwrap())
            .collect();
        assert_eq!(roles, vec!["user", "model", "user", "model"]);
    }

    #[test]
    fn test_convert_s8_assistant_text_and_tool_call_same_turn() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::Text {
                    text: "Let me search for that.".to_string(),
                },
                CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "web_search".to_string(),
                    arguments: serde_json::json!({"q": "test"}),
                },
            ],
        }];
        let result = GeminiProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Some("model".to_string()));
        assert_eq!(result[0].parts.len(), 2);

        // First part should be text
        match &result[0].parts[0] {
            Part::Text { text } => assert_eq!(text, "Let me search for that."),
            _ => panic!("Expected Text part"),
        }
        // Second part should be function call
        let json = serde_json::to_value(&result[0]).unwrap();
        let parts = json["parts"].as_array().unwrap();
        assert!(parts[1].get("functionCall").is_some());
        assert_eq!(parts[1]["functionCall"]["name"], "web_search");
    }
}
