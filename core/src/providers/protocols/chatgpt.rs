//! Codex Responses API protocol adapter
//!
//! Handles the Codex backend API format at chatgpt.com/backend-api/codex/responses.
//! Uses the Responses API wire format with typed SSE streaming events.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason, TokenUsage};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::chatgpt::types::{
    FunctionToolDef, InputItem, ReasoningConfig, ResponseResource, ResponsesRequest, StreamEvent,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use reqwest::Client;
use tracing::{debug, error, warn};

const CODEX_ENDPOINT: &str = "/backend-api/codex/responses";

/// Codex Responses API protocol adapter
///
/// Translates between Aleph's unified request format and the Codex
/// Responses API format used by chatgpt.com/backend-api/codex/responses.
pub struct ChatGptProtocol {
    client: Client,
}

impl ChatGptProtocol {
    /// Create a new Codex protocol adapter with the given HTTP client
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL from provider configuration
    fn build_endpoint(config: &ProviderConfig) -> String {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://chatgpt.com".to_string());
        format!("{}{}", base_url, CODEX_ENDPOINT)
    }

    /// Map Aleph ThinkLevel to Responses API reasoning config
    fn build_reasoning(payload: &RequestPayload) -> Option<ReasoningConfig> {
        use crate::agents::thinking::ThinkLevel;
        match payload.think_level {
            Some(ThinkLevel::Low) => Some(ReasoningConfig {
                effort: Some("low".to_string()),
                summary: Some("auto".to_string()),
            }),
            Some(ThinkLevel::Medium) => Some(ReasoningConfig {
                effort: Some("medium".to_string()),
                summary: Some("auto".to_string()),
            }),
            Some(ThinkLevel::High) => Some(ReasoningConfig {
                effort: Some("high".to_string()),
                summary: Some("auto".to_string()),
            }),
            _ => None,
        }
    }

    /// Convert UnifiedMessages to Responses API InputItems
    fn convert_messages(messages: &[UnifiedMessage]) -> Vec<InputItem> {
        let mut items = Vec::new();
        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    let text = content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    items.push(InputItem::Message {
                        role: "user".to_string(),
                        content: text,
                    });
                }
                UnifiedMessage::Assistant { content } => {
                    let text: String = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        items.push(InputItem::Message {
                            role: "assistant".to_string(),
                            content: text,
                        });
                    }
                    for block in content {
                        if let ContentBlock::ToolCall { id, name, arguments } = block {
                            items.push(InputItem::FunctionCall {
                                call_id: id.clone(),
                                name: name.clone(),
                                arguments: serde_json::to_string(arguments).unwrap_or_default(),
                            });
                        }
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
                            ContentBlock::Text { text } => text.clone(),
                            ContentBlock::Json { value } => {
                                serde_json::to_string(value).unwrap_or_default()
                            }
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    items.push(InputItem::FunctionCallOutput {
                        call_id: tool_call_id.clone(),
                        output,
                    });
                }
            }
        }
        items
    }

    /// Build a Responses API request from the unified payload
    ///
    /// Parses XML-tagged conversation history from the input string into
    /// native Responses API InputItems. This allows multi-turn tool calling
    /// to work correctly with the Codex API.
    pub fn build_responses_request(
        payload: &RequestPayload,
        model: &str,
    ) -> ResponsesRequest {
        let input = Self::convert_messages(payload.messages);

        // Convert tool definitions to Responses API format
        let tools = payload.tools.map(|tool_defs| {
            tool_defs
                .iter()
                .map(|td| {
                    // Ensure parameters schema has required "properties" field
                    // OpenAI rejects object schemas without it
                    let mut params = td.parameters.clone();
                    if let Some(obj) = params.as_object_mut() {
                        if !obj.contains_key("properties") {
                            obj.insert(
                                "properties".to_string(),
                                serde_json::Value::Object(serde_json::Map::new()),
                            );
                        }
                    }
                    FunctionToolDef {
                        tool_type: "function".to_string(),
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: params,
                    }
                })
                .collect()
        });

        ResponsesRequest {
            model: model.to_string(),
            input,
            instructions: payload.system_prompt.map(|s| s.to_string()),
            stream: true,
            store: false,
            reasoning: Self::build_reasoning(payload),
            tools,
        }
    }

    /// Extract text content from a completed ResponseResource
    fn extract_text(response: &ResponseResource) -> Option<String> {
        let mut texts = Vec::new();
        for item in &response.output {
            if let crate::providers::chatgpt::types::OutputItem::Message { content, .. } = item {
                for part in content {
                    if !part.text.is_empty() {
                        texts.push(part.text.clone());
                    }
                }
            }
        }
        if texts.is_empty() {
            None
        } else {
            Some(texts.join(""))
        }
    }

    /// Parse the XML-tagged input string into native Responses API InputItems.
    ///
    /// Recognizes these tags from `AiProviderBridge::format_messages`:
    /// - `<user>...</user>` → Message { role: "user" }
    /// - `<assistant>...</assistant>` → Message { role: "assistant" }
    /// - `<tool_use id="..." name="...">...</tool_use>` → FunctionCall
    /// - `<tool_result id="...">...</tool_result>` → FunctionCallOutput
    /// - `<tool_error id="...">...</tool_error>` → FunctionCallOutput (with error prefix)
    fn parse_input_items(input: &str) -> Vec<InputItem> {
        let mut items = Vec::new();
        debug!("parse_input_items: input length={}", input.len());
        let mut remaining = input;

        while !remaining.is_empty() {
            // Find the next opening tag
            let Some(tag_start) = remaining.find('<') else {
                // No more tags — remaining text is plain content, skip it
                break;
            };

            // Skip any text before the tag
            remaining = &remaining[tag_start..];

            // Try each known tag type
            if let Some(item) = Self::try_parse_tag(&mut remaining, "user") {
                items.push(item);
            } else if let Some(item) = Self::try_parse_tag(&mut remaining, "assistant") {
                items.push(item);
            } else if let Some(item) = Self::try_parse_tag(&mut remaining, "tool_use") {
                items.push(item);
            } else if let Some(item) = Self::try_parse_tag(&mut remaining, "tool_result") {
                items.push(item);
            } else if let Some(item) = Self::try_parse_tag(&mut remaining, "tool_error") {
                items.push(item);
            } else {
                // Unknown tag or malformed — skip past '<'
                remaining = &remaining[1..];
            }
        }

        // Fallback: if no tags were found, treat the whole input as a user message
        if items.is_empty() {
            items.push(InputItem::Message {
                role: "user".to_string(),
                content: input.to_string(),
            });
        }

        items
    }

    /// Try to parse a specific XML tag from the start of `remaining`.
    /// On success, advances `remaining` past the closing tag and returns the InputItem.
    fn try_parse_tag(remaining: &mut &str, tag: &str) -> Option<InputItem> {
        let open_prefix = format!("<{}", tag);
        if !remaining.starts_with(&open_prefix) {
            return None;
        }

        // Find end of opening tag '>'
        let after_tag_name = &remaining[open_prefix.len()..];
        let gt_pos = after_tag_name.find('>')?;
        let attrs_str = &after_tag_name[..gt_pos].trim();
        let after_open = &after_tag_name[gt_pos + 1..];

        // Find closing tag
        let close_tag = format!("</{}>", tag);
        let close_pos = after_open.find(&close_tag)?;
        let content = after_open[..close_pos].trim();

        // Advance remaining past the closing tag
        *remaining = &after_open[close_pos + close_tag.len()..];

        // Parse attributes (id="..." name="...")
        let parse_attr = |key: &str| -> String {
            let needle = format!("{}=\"", key);
            if let Some(start) = attrs_str.find(&needle) {
                let val_start = start + needle.len();
                if let Some(end) = attrs_str[val_start..].find('"') {
                    return attrs_str[val_start..val_start + end].to_string();
                }
            }
            String::new()
        };

        match tag {
            "user" => Some(InputItem::Message {
                role: "user".to_string(),
                content: content.to_string(),
            }),
            "assistant" => Some(InputItem::Message {
                role: "assistant".to_string(),
                content: content.to_string(),
            }),
            "tool_use" => Some(InputItem::FunctionCall {
                call_id: parse_attr("id"),
                name: parse_attr("name"),
                arguments: content.to_string(),
            }),
            "tool_result" => Some(InputItem::FunctionCallOutput {
                call_id: parse_attr("id"),
                output: content.to_string(),
            }),
            "tool_error" => Some(InputItem::FunctionCallOutput {
                call_id: parse_attr("id"),
                output: format!("ERROR: {}", content),
            }),
            _ => None,
        }
    }

    /// Extract native tool calls from a completed ResponseResource
    fn extract_tool_calls(response: &ResponseResource) -> Vec<NativeToolCall> {
        let mut calls = Vec::new();
        for item in &response.output {
            if let crate::providers::chatgpt::types::OutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } = item
            {
                debug!("Codex function_call: name={} call_id={} arguments={}", name, call_id, arguments);
                let args = serde_json::from_str(arguments)
                    .unwrap_or_else(|_| serde_json::Value::String(arguments.clone()));
                calls.push(NativeToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: args,
                });
            }
        }
        calls
    }

    /// Parse a single SSE data line into a StreamEvent
    fn parse_sse_data(data: &str) -> Option<StreamEvent> {
        if data == "[DONE]" {
            return None;
        }
        serde_json::from_str(data).ok()
    }
}

#[async_trait]
impl ProtocolAdapter for ChatGptProtocol {
    fn supports_native_tools(&self) -> bool {
        true
    }

    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        _is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let request = Self::build_responses_request(payload, config.default_model());

        let access_token = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("Codex access token not set — run OAuth login first")
        })?;

        debug!(
            endpoint = %endpoint,
            model = %config.default_model(),
            "Building Codex Responses API request"
        );

        let builder = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request);

        Ok(builder)
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "Codex API error");
            if status.as_u16() == 401 {
                return Err(AlephError::provider(
                    "Codex authentication expired — please re-login",
                ));
            }
            if status.as_u16() == 429 {
                return Err(AlephError::provider(
                    "Codex subscription rate limit reached — please try again later",
                ));
            }
            return Err(AlephError::provider(format!(
                "Codex API error ({}): {}",
                status, error_text
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to read Codex response: {}", e)))?;

        // Parse SSE events, looking for the Completed event with full response
        let mut result = String::new();
        let mut tool_calls: Vec<NativeToolCall> = Vec::new();
        for line in text.lines() {
            let data = if let Some(d) = line.strip_prefix("data: ") {
                d
            } else {
                continue;
            };
            if let Some(event) = Self::parse_sse_data(data) {
                match event {
                    StreamEvent::TextDelta { delta, .. } => {
                        result.push_str(&delta);
                    }
                    StreamEvent::Completed { ref response } => {
                        // Prefer extracting from completed response for accuracy
                        if let Some(full_text) = Self::extract_text(response) {
                            result = full_text;
                        }
                        // Extract function calls
                        tool_calls = Self::extract_tool_calls(response);
                    }
                    StreamEvent::Failed { response } => {
                        let msg = response
                            .error
                            .map(|e| format!("{}: {}", e.code, e.message))
                            .unwrap_or_else(|| "Unknown error".to_string());
                        return Err(AlephError::provider(format!("Codex request failed: {}", msg)));
                    }
                    _ => {}
                }
            }
        }

        if !tool_calls.is_empty() {
            Ok(ProviderResponse {
                text: if result.is_empty() { None } else { Some(result) },
                tool_calls,
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            })
        } else if result.is_empty() {
            Err(AlephError::provider("Empty response from Codex"))
        } else {
            Ok(ProviderResponse::text_only(result))
        }
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Codex API error ({}): {}",
                status, error_text
            )));
        }

        // Buffer for incomplete SSE lines across chunks
        let line_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        let stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .try_filter_map(move |chunk| {
                let buf = line_buf.clone();
                async move {
                    let text = std::str::from_utf8(&chunk)
                        .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                    let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                    buf_guard.push_str(text);

                    let mut delta = String::new();

                    // Process complete lines from buffer
                    while let Some(newline_pos) = buf_guard.find('\n') {
                        let line = buf_guard[..newline_pos].trim_end().to_string();
                        buf_guard.drain(..=newline_pos);

                        let data = if let Some(d) = line.strip_prefix("data: ") {
                            d
                        } else {
                            continue;
                        };

                        if let Some(event) = Self::parse_sse_data(data) {
                            match event {
                                StreamEvent::TextDelta { delta: d, .. } => {
                                    delta.push_str(&d);
                                }
                                StreamEvent::Failed { response } => {
                                    let msg = response
                                        .error
                                        .map(|e| format!("{}: {}", e.code, e.message))
                                        .unwrap_or_else(|| "Unknown error".to_string());
                                    warn!(error = %msg, "Codex stream failed");
                                }
                                _ => {}
                            }
                        }
                    }

                    if delta.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(delta))
                    }
                }
            });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "chatgpt"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_responses_request_basic() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let request = ChatGptProtocol::build_responses_request(&payload, "codex-mini-latest");

        assert_eq!(request.model, "codex-mini-latest");
        assert!(!request.store);
        assert!(request.stream);
        assert!(request.instructions.is_none());
        assert!(request.reasoning.is_none());
        assert_eq!(request.input.len(), 1);
        match &request.input[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content, "Hello");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_build_responses_request_with_system_prompt() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("You are helpful"));
        let request = ChatGptProtocol::build_responses_request(&payload, "codex-mini-latest");

        assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
        match &request.input[0] {
            InputItem::Message { content, .. } => {
                assert_eq!(content, "Hello");
                assert!(!content.contains("You are helpful"));
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_build_responses_request_with_reasoning() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Think about this")];
        let payload = RequestPayload::new(&msgs)
            .with_think_level(Some(ThinkLevel::High));
        let request = ChatGptProtocol::build_responses_request(&payload, "codex-mini-latest");

        let reasoning = request.reasoning.unwrap();
        assert_eq!(reasoning.effort.as_deref(), Some("high"));
        assert_eq!(reasoning.summary.as_deref(), Some("auto"));
    }

    #[test]
    fn test_parse_sse_data_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
        let event = ChatGptProtocol::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_done() {
        let result = ChatGptProtocol::parse_sse_data("[DONE]");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_completed() {
        let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
        let event = ChatGptProtocol::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                let text = ChatGptProtocol::extract_text(&response);
                assert_eq!(text, Some("Hello world".to_string()));
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_text_from_response() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![crate::providers::chatgpt::types::OutputItem::Message {
                id: "msg_1".to_string(),
                role: "assistant".to_string(),
                content: vec![crate::providers::chatgpt::types::ContentPart {
                    part_type: "output_text".to_string(),
                    text: "Test output".to_string(),
                }],
            }],
            usage: None,
            error: None,
        };
        assert_eq!(
            ChatGptProtocol::extract_text(&response),
            Some("Test output".to_string())
        );
    }

    #[test]
    fn test_extract_text_empty_output() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![],
            usage: None,
            error: None,
        };
        assert_eq!(ChatGptProtocol::extract_text(&response), None);
    }

    #[test]
    fn test_adapter_name() {
        let adapter = ChatGptProtocol::new(Client::new());
        assert_eq!(adapter.name(), "chatgpt");
    }

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("codex-mini-latest");
        let endpoint = ChatGptProtocol::build_endpoint(&config);
        assert!(endpoint.ends_with("/backend-api/codex/responses"));
    }

    #[test]
    fn test_create_provider_via_factory() {
        use crate::config::ProviderConfig;
        use crate::providers::create_provider;

        let mut config = ProviderConfig::test_config("codex-mini-latest");
        config.protocol = Some("chatgpt".to_string());
        config.api_key = Some("test_token".to_string());
        config.base_url = Some("https://chatgpt.com".to_string());
        config.enabled = true;

        let provider = create_provider("chatgpt-sub", config);
        assert!(
            provider.is_ok(),
            "Should create chatgpt provider: {:?}",
            provider.err()
        );
    }

    #[test]
    fn test_chatgpt_preset() {
        use crate::providers::presets::get_preset;

        let preset = get_preset("chatgpt");
        assert!(preset.is_some(), "chatgpt preset should exist");

        let p = preset.unwrap();
        assert_eq!(p.protocol, "chatgpt");
        assert_eq!(p.default_model, "gpt-5.4");
    }

    // ─── convert_messages tests ─────────────────────────────────────

    #[test]
    fn test_convert_s1_pure_text_user_message() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("hello")];
        let items = ChatGptProtocol::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_s2_multi_turn_conversation() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems programming language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let items = ChatGptProtocol::convert_messages(&msgs);

        assert_eq!(items.len(), 3);
        match &items[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content, "What is Rust?");
            }
            other => panic!("Expected Message, got {:?}", other),
        }
        match &items[1] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "assistant");
                assert_eq!(content, "Rust is a systems programming language.");
            }
            other => panic!("Expected Message, got {:?}", other),
        }
        match &items[2] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content, "Tell me more.");
            }
            other => panic!("Expected Message, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s3_assistant_text_and_tool_call() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "Let me search for that.".to_string(),
                },
                ContentBlock::ToolCall {
                    id: "call_abc".to_string(),
                    name: "web_search".to_string(),
                    arguments: serde_json::json!({"query": "rust lang"}),
                },
            ],
        }];
        let items = ChatGptProtocol::convert_messages(&msgs);

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "assistant".to_string(),
                content: "Let me search for that.".to_string(),
            }
        );
        match &items[1] {
            InputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(name, "web_search");
                let parsed: serde_json::Value = serde_json::from_str(arguments).unwrap();
                assert_eq!(parsed, serde_json::json!({"query": "rust lang"}));
            }
            other => panic!("Expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s4_tool_result() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::tool_result(
            "call_123",
            "search",
            "Found 5 results",
            false,
        )];
        let items = ChatGptProtocol::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::FunctionCallOutput {
                call_id: "call_123".to_string(),
                output: "Found 5 results".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_s5_full_tool_use_cycle() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        let msgs = [
            UnifiedMessage::user("Search for Rust tutorials"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "rust tutorials"}),
                }],
            },
            UnifiedMessage::tool_result("call_1", "search", "Tutorial list: ...", false),
            UnifiedMessage::assistant("Here are some Rust tutorials I found."),
        ];
        let items = ChatGptProtocol::convert_messages(&msgs);

        // User(1) + FunctionCall(1) + FunctionCallOutput(1) + Assistant Message(1) = 4
        // Note: Assistant with only ToolCall has no text, so no Message emitted for it
        assert_eq!(items.len(), 4);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: "Search for Rust tutorials".to_string(),
            }
        );
        match &items[1] {
            InputItem::FunctionCall { call_id, name, .. } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "search");
            }
            other => panic!("Expected FunctionCall, got {:?}", other),
        }
        assert_eq!(
            items[2],
            InputItem::FunctionCallOutput {
                call_id: "call_1".to_string(),
                output: "Tutorial list: ...".to_string(),
            }
        );
        assert_eq!(
            items[3],
            InputItem::Message {
                role: "assistant".to_string(),
                content: "Here are some Rust tutorials I found.".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_s6_multiple_tool_calls_one_turn() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "Running multiple searches.".to_string(),
                },
                ContentBlock::ToolCall {
                    id: "c1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "a"}),
                },
                ContentBlock::ToolCall {
                    id: "c2".to_string(),
                    name: "fetch".to_string(),
                    arguments: serde_json::json!({"url": "http://example.com"}),
                },
                ContentBlock::ToolCall {
                    id: "c3".to_string(),
                    name: "calc".to_string(),
                    arguments: serde_json::json!({"expr": "1+1"}),
                },
            ],
        }];
        let items = ChatGptProtocol::convert_messages(&msgs);

        // 1 Message (text) + 3 FunctionCalls = 4
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[0], InputItem::Message { role, .. } if role == "assistant"));
        assert!(matches!(&items[1], InputItem::FunctionCall { call_id, .. } if call_id == "c1"));
        assert!(matches!(&items[2], InputItem::FunctionCall { call_id, .. } if call_id == "c2"));
        assert!(matches!(&items[3], InputItem::FunctionCall { call_id, .. } if call_id == "c3"));
    }

    #[test]
    fn test_convert_s7_error_tool_result() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::tool_result(
            "call_err",
            "dangerous_tool",
            "Permission denied",
            true, // is_error = true
        )];
        let items = ChatGptProtocol::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        // convert_messages does not distinguish is_error — it just passes content through
        assert_eq!(
            items[0],
            InputItem::FunctionCallOutput {
                call_id: "call_err".to_string(),
                output: "Permission denied".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_s8_json_tool_output() {
        use crate::providers::message::UnifiedMessage;
        let json_val = serde_json::json!({"results": [1, 2, 3], "total": 3});
        let msgs = [UnifiedMessage::tool_result_json(
            "call_json",
            "api_call",
            json_val.clone(),
            false,
        )];
        let items = ChatGptProtocol::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        match &items[0] {
            InputItem::FunctionCallOutput { call_id, output } => {
                assert_eq!(call_id, "call_json");
                let parsed: serde_json::Value = serde_json::from_str(output).unwrap();
                assert_eq!(parsed, json_val);
            }
            other => panic!("Expected FunctionCallOutput, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s9_completed_event_usage_extraction() {
        // Test that parse_sse_data + extract on a Completed event with usage works
        let data = r#"{"type":"response.completed","response":{"id":"resp_u","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
        let event = ChatGptProtocol::parse_sse_data(data).unwrap();
        match event {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                assert_eq!(
                    ChatGptProtocol::extract_text(&response),
                    Some("done".to_string())
                );
                let usage = response.usage.as_ref().expect("usage should be present");
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
                assert_eq!(usage.total_tokens, 15);
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s10_incomplete_status() {
        // Test that an "incomplete" status is correctly deserialized from a Completed event
        // (the protocol maps incomplete → MaxTokens in parse_response, but we verify
        // the status field is correctly captured here)
        let data = r#"{"type":"response.completed","response":{"id":"resp_inc","status":"incomplete","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"partial"}]}]}}"#;
        let event = ChatGptProtocol::parse_sse_data(data).unwrap();
        match event {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "incomplete");
                assert_eq!(
                    ChatGptProtocol::extract_text(&response),
                    Some("partial".to_string())
                );
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }
}
