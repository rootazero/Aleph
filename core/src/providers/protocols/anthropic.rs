//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    ProtocolAdapter, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::anthropic::{
    AnthropicTool, ContentBlock, ImageSource, Message,
    MessageContent, MessagesRequest, SystemBlock, ThinkingBlock,
};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use crate::providers::message::UnifiedMessage;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use std::collections::VecDeque;
use tracing::{debug, warn};

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

    /// Build the comma-separated anthropic-beta header value for a given model.
    ///
    /// Always includes interleaved-thinking and fine-grained-tool-streaming.
    /// Adds the 128k output beta for large context models (opus-4, sonnet-4).
    fn build_beta_headers(model: &str) -> String {
        let mut betas = vec![
            "interleaved-thinking-2025-05-14",
            "fine-grained-tool-streaming-2025-05-14",
        ];
        if Self::is_large_context_model(model) {
            betas.push("output-128k-2025-02-19");
        }
        betas.join(",")
    }

    /// Returns true for large context models that support 128k output tokens.
    fn is_large_context_model(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("opus-4") || m.contains("sonnet-4")
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

}

#[async_trait]
impl ProtocolAdapter for AnthropicProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
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
                budget_tokens: Some(budget),
                display: None,
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

        // Mark the last system block for ephemeral prompt caching
        let system = payload
            .system_prompt
            .map(|s| vec![SystemBlock::cached_text(s)]);

        let request_body = MessagesRequest {
            model: config.default_model().to_string(),
            messages,
            max_tokens,
            system,
            temperature,
            stream: Some(true), // always streaming (stream-first architecture)
            thinking,
            tools,
            service_tier: None,
            output_config: None,
        };

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.default_model(),
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
            .header("anthropic-beta", Self::build_beta_headers(config.default_model()))
            .header("Content-Type", "application/json")
            .json(&body))
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    /// Stream fine-grained delta events from the Anthropic Messages streaming API.
    ///
    /// Parses Anthropic SSE events and emits fine-grained [`ProviderDelta`] events.
    /// Uses the unfold+pending-queue pattern so that multi-delta events (e.g.
    /// message_delta with stop_reason + usage) can emit all deltas without loss.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Anthropic API error ({}): {}",
                status, error_text
            )));
        }

        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .boxed();

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: futures::stream::BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE line buffer (handles chunk boundaries)
            line_buf: String,
            /// Maps content_block index (u32) → tool_use id
            block_ids: IndexIdTracker,
            /// Pending deltas queued from multi-delta events
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: String::new(),
            block_ids: IndexIdTracker::new(),
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
                            parse_anthropic_sse_event(
                                data,
                                &mut state.block_ids,
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
                                    parse_anthropic_sse_event(
                                        data,
                                        &mut state.block_ids,
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
                    Some(Ok(chunk)) => {
                        match std::str::from_utf8(&chunk) {
                            Ok(text) => state.line_buf.push_str(text),
                            Err(e) => {
                                state.done = true;
                                return Some((
                                    Err(AlephError::provider(format!("UTF-8 error: {}", e))),
                                    state,
                                ));
                            }
                        }
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }
}

// =============================================================================
// SSE parsing helper for Anthropic Messages streaming format
// =============================================================================

/// Parse one SSE data line from the Anthropic Messages stream and push
/// zero or more [`ProviderDelta`] events into `out`.
///
/// Anthropic SSE event types handled:
/// - `content_block_start`: tracks block types; emits `ToolCallStart` for tool_use blocks
/// - `content_block_delta`: emits `TextDelta`, `ThinkingDelta`, or `ToolCallArgDelta`
/// - `content_block_stop`: emits `ToolCallEnd` for tool_use blocks
/// - `message_delta`: emits `Usage` and `Done` from stop_reason
/// - `error`: emits `Error`
pub fn parse_anthropic_sse_event(
    data: &str,
    block_ids: &mut IndexIdTracker,
    out: &mut VecDeque<Result<ProviderDelta>>,
) {
    let v: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, data = %data, "Failed to parse Anthropic SSE event");
            return;
        }
    };

    let event_type = match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return,
    };

    match event_type {
        // ── content_block_start ───────────────────────────────────────────────
        "content_block_start" => {
            let index = v
                .get("index")
                .and_then(|i| i.as_u64())
                .unwrap_or(0);
            let block = match v.get("content_block") {
                Some(b) => b,
                None => return,
            };
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if block_type == "tool_use" {
                let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                // Track index → id for subsequent input_json_delta events
                block_ids.track(index, id.to_string());
                out.push_back(Ok(ProviderDelta::ToolCallStart {
                    id: id.to_string(),
                    name: name.to_string(),
                }));
            }
            // text and thinking blocks: no delta emitted at start
        }

        // ── content_block_delta ───────────────────────────────────────────────
        "content_block_delta" => {
            let index = v
                .get("index")
                .and_then(|i| i.as_u64())
                .unwrap_or(0);
            let delta = match v.get("delta") {
                Some(d) => d,
                None => return,
            };
            let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match delta_type {
                "text_delta" => {
                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                        out.push_back(Ok(ProviderDelta::TextDelta(text.to_string())));
                    }
                }
                "thinking_delta" => {
                    if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                        out.push_back(Ok(ProviderDelta::ThinkingDelta(thinking.to_string())));
                    }
                }
                "input_json_delta" => {
                    // partial_json fragment for tool_use argument streaming
                    if let Some(partial) =
                        delta.get("partial_json").and_then(|p| p.as_str())
                    {
                        if let Some(call_id) = block_ids.get(index) {
                            out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                                id: call_id.to_string(),
                                delta: partial.to_string(),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        // ── content_block_stop ────────────────────────────────────────────────
        "content_block_stop" => {
            let index = v
                .get("index")
                .and_then(|i| i.as_u64())
                .unwrap_or(0);
            // Only emit ToolCallEnd if this index was a tool_use block
            if let Some(call_id) = block_ids.get(index) {
                out.push_back(Ok(ProviderDelta::ToolCallEnd {
                    id: call_id.to_string(),
                }));
            }
        }

        // ── message_delta ─────────────────────────────────────────────────────
        "message_delta" => {
            // Extract stop_reason
            let stop_reason = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|r| r.as_str());

            // Extract usage (output_tokens for the delta portion)
            if let Some(usage) = v.get("usage") {
                let output = usage
                    .get("output_tokens")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32;
                let cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|t| t.as_u64())
                    .map(|t| t as u32);
                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens: 0, // input usage is in message_start, not message_delta
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    thinking_tokens: None,
                })));
            }

            if let Some(reason) = stop_reason {
                let sr = match reason {
                    "end_turn" => StopReason::EndTurn,
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::MaxTokens,
                    _ => StopReason::Unknown,
                };
                out.push_back(Ok(ProviderDelta::Done(sr)));
            }
        }

        // ── message_stop ──────────────────────────────────────────────────────
        "message_stop" => {
            // Stream is ending; no additional deltas needed (Done was emitted at message_delta)
        }

        // ── error ─────────────────────────────────────────────────────────────
        "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Anthropic error");
            out.push_back(Ok(ProviderDelta::Error(message.to_string())));
        }

        // ── message_start / ping / other ──────────────────────────────────────
        _ => {
            // message_start contains initial usage; currently not extracted
        }
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

        let request = protocol.build_request(&payload, &config).unwrap();
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

        let request = protocol.build_request(&payload, &config).unwrap();
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

    #[test]
    fn test_thinking_block_enabled_serialization() {
        use crate::providers::anthropic::types::ThinkingBlock;
        let block = ThinkingBlock {
            thinking_type: "enabled".to_string(),
            budget_tokens: Some(10000),
            display: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "enabled");
        assert_eq!(json["budget_tokens"], 10000);
        assert!(json.get("display").is_none());
    }

    #[test]
    fn test_thinking_block_adaptive_serialization() {
        use crate::providers::anthropic::types::ThinkingBlock;
        let block = ThinkingBlock {
            thinking_type: "adaptive".to_string(),
            budget_tokens: None,
            display: Some("summarized".to_string()),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "adaptive");
        assert!(json.get("budget_tokens").is_none());
        assert_eq!(json["display"], "summarized");
    }
}

// =============================================================================
// Stream delta parsing tests
// =============================================================================

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::providers::delta::ProviderDelta;

    // Helper: run parse_anthropic_sse_event on a raw JSON string (without "data: " prefix)
    fn parse(data: &str) -> Vec<ProviderDelta> {
        let mut block_ids = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        parse_anthropic_sse_event(data, &mut block_ids, &mut pending);
        pending.into_iter().map(|r| r.unwrap()).collect()
    }

    // Helper: run a sequence of SSE data payloads through the parser with shared state
    fn parse_sequence(events: &[&str]) -> Vec<ProviderDelta> {
        let mut block_ids = IndexIdTracker::new();
        let mut all = Vec::new();
        for data in events {
            let mut pending = VecDeque::new();
            parse_anthropic_sse_event(data, &mut block_ids, &mut pending);
            all.extend(pending.into_iter().map(|r| r.unwrap()));
        }
        all
    }

    // ── Test 1: Text-only response ──────────────────────────────────────────

    #[test]
    fn test_text_only_response() {
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let deltas = parse_sequence(&events);

        // Should have TextDelta("Hello"), Usage, Done(EndTurn)
        let text_deltas: Vec<_> = deltas
            .iter()
            .filter_map(|d| match d {
                ProviderDelta::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas, vec!["Hello"]);

        let done = deltas.iter().find(|d| matches!(d, ProviderDelta::Done(_)));
        assert!(matches!(done, Some(ProviderDelta::Done(StopReason::EndTurn))));
    }

    // ── Test 2: Tool use response ───────────────────────────────────────────

    #[test]
    fn test_tool_use_response() {
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"search","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"rust\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":20}}"#,
        ];
        let deltas = parse_sequence(&events);

        // ToolCallStart
        let start = deltas.iter().find(|d| matches!(d, ProviderDelta::ToolCallStart { .. }));
        assert!(matches!(
            start,
            Some(ProviderDelta::ToolCallStart { id, name }) if id == "toolu_1" && name == "search"
        ));

        // ToolCallArgDelta
        let arg_delta = deltas.iter().find(|d| matches!(d, ProviderDelta::ToolCallArgDelta { .. }));
        assert!(matches!(
            arg_delta,
            Some(ProviderDelta::ToolCallArgDelta { id, delta }) if id == "toolu_1" && delta.contains("rust")
        ));

        // ToolCallEnd
        let end = deltas.iter().find(|d| matches!(d, ProviderDelta::ToolCallEnd { .. }));
        assert!(matches!(
            end,
            Some(ProviderDelta::ToolCallEnd { id }) if id == "toolu_1"
        ));

        // Done(ToolUse)
        let done = deltas.iter().find(|d| matches!(d, ProviderDelta::Done(_)));
        assert!(matches!(done, Some(ProviderDelta::Done(StopReason::ToolUse))));
    }

    // ── Test 3: Thinking + text response ───────────────────────────────────

    #[test]
    fn test_thinking_response() {
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Answer"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":10}}"#,
        ];
        let deltas = parse_sequence(&events);

        let thinking = deltas.iter().find(|d| matches!(d, ProviderDelta::ThinkingDelta(_)));
        assert!(matches!(
            thinking,
            Some(ProviderDelta::ThinkingDelta(t)) if t == "Let me think"
        ));

        let text = deltas.iter().find(|d| matches!(d, ProviderDelta::TextDelta(_)));
        assert!(matches!(
            text,
            Some(ProviderDelta::TextDelta(t)) if t == "Answer"
        ));

        let done = deltas.iter().find(|d| matches!(d, ProviderDelta::Done(_)));
        assert!(matches!(done, Some(ProviderDelta::Done(StopReason::EndTurn))));
    }

    // ── Test 4: Beta headers ────────────────────────────────────────────────

    #[test]
    fn test_beta_headers_standard_model() {
        let headers = AnthropicProtocol::build_beta_headers("claude-3-5-sonnet-20241022");
        // Should include the two always-on betas
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        assert!(headers.contains("fine-grained-tool-streaming-2025-05-14"));
        // Standard model should NOT have 128k output beta
        assert!(!headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_beta_headers_opus4_model() {
        let headers = AnthropicProtocol::build_beta_headers("claude-opus-4-20250514");
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        assert!(headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_beta_headers_sonnet4_model() {
        let headers = AnthropicProtocol::build_beta_headers("claude-sonnet-4-5");
        assert!(headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_is_large_context_model() {
        assert!(AnthropicProtocol::is_large_context_model("claude-opus-4-20250514"));
        assert!(AnthropicProtocol::is_large_context_model("claude-sonnet-4-5"));
        assert!(!AnthropicProtocol::is_large_context_model("claude-3-5-sonnet-20241022"));
        assert!(!AnthropicProtocol::is_large_context_model("claude-3-opus-20240229"));
    }

    // ── Test 5: Error event ─────────────────────────────────────────────────

    #[test]
    fn test_error_event() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let deltas = parse(data);
        assert_eq!(deltas.len(), 1);
        assert!(matches!(&deltas[0], ProviderDelta::Error(msg) if msg == "Overloaded"));
    }

    // ── Test 6: Usage in message_delta ─────────────────────────────────────

    #[test]
    fn test_message_delta_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":42}}"#;
        let deltas = parse(data);

        let usage = deltas.iter().find(|d| matches!(d, ProviderDelta::Usage(_)));
        assert!(matches!(
            usage,
            Some(ProviderDelta::Usage(TokenUsage { output_tokens: 42, .. }))
        ));
    }

    // ── Test 7: content_block_stop does not emit ToolCallEnd for text blocks ─

    #[test]
    fn test_text_block_stop_no_tool_call_end() {
        // Process a text block start + stop: should NOT produce ToolCallEnd
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ];
        let deltas = parse_sequence(&events);
        // There should be no ToolCallEnd
        assert!(!deltas.iter().any(|d| matches!(d, ProviderDelta::ToolCallEnd { .. })));
    }

    // ── Test 8: prompt caching in build_request ─────────────────────────────

    #[test]
    fn test_build_request_system_block_cached() {
        use crate::providers::message::UnifiedMessage;
        let protocol = AnthropicProtocol::new(reqwest::Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("Be helpful."));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();

        // system block should have cache_control with type=ephemeral
        let system = &body["system"];
        assert!(system.is_array());
        let first_block = &system[0];
        assert_eq!(first_block["type"], "text");
        assert_eq!(first_block["text"], "Be helpful.");
        assert_eq!(first_block["cache_control"]["type"], "ephemeral");
    }

    // ── Test 9: beta header in built request ────────────────────────────────

    #[test]
    fn test_build_request_beta_header_present() {
        use crate::providers::message::UnifiedMessage;
        let protocol = AnthropicProtocol::new(reqwest::Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let beta_header = built
            .headers()
            .get("anthropic-beta")
            .and_then(|v| v.to_str().ok());
        assert!(beta_header.is_some());
        assert!(beta_header.unwrap().contains("interleaved-thinking-2025-05-14"));
    }
}
