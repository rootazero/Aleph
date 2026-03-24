//! OpenAI Responses API protocol adapter
//!
//! Handles both the standard OpenAI Responses API at /v1/responses and the
//! Codex variant at /backend-api/codex/responses via `ResponsesVariant`.

use std::collections::HashMap;

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason, TokenUsage};
use crate::providers::delta::ProviderDelta;
use crate::providers::responses::shared;
use crate::providers::responses::types::{ContextManagement, ResponsesRequest, StreamEvent, TextConfig};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use tracing::{debug, warn};

// =============================================================================
// ResponsesVariant
// =============================================================================

/// Variant-specific configuration for the Responses API adapter.
///
/// Allows the same adapter to be used for standard OpenAI (`Default`) and
/// Codex (`ResponsesVariant::codex()`), which share the same wire format but
/// differ in endpoint path, request fields, and auth headers.
#[derive(Debug, Clone, Default)]
pub struct ResponsesVariant {
    /// Override the default /v1/responses endpoint path
    pub endpoint_path: Option<String>,
    /// Extra HTTP headers to add to every request
    pub extra_headers: Vec<(String, String)>,
    /// Force store field value (None = let auto-detection decide)
    pub store: Option<bool>,
    /// Text output configuration
    pub text: Option<TextConfig>,
    /// Additional fields to include in response
    pub include: Option<Vec<String>>,
}

impl ResponsesVariant {
    /// Create a Codex-specific variant configuration.
    ///
    /// Sets the Codex backend endpoint path and required Codex mode fields
    /// (store=false, verbosity=medium, reasoning.encrypted_content include).
    pub fn codex() -> Self {
        Self {
            endpoint_path: Some("/backend-api/codex/responses".into()),
            store: Some(false),
            text: Some(TextConfig {
                format: None,
                verbosity: Some("medium".into()),
            }),
            include: Some(vec!["reasoning.encrypted_content".into()]),
            ..Default::default()
        }
    }
}

// =============================================================================
// OpenAiResponsesProtocol
// =============================================================================

/// OpenAI Responses API protocol adapter
///
/// Translates between Aleph's unified request format and the OpenAI Responses
/// API wire format. Supports both standard OpenAI and Codex endpoints via
/// `ResponsesVariant`.
pub struct OpenAiResponsesProtocol {
    client: Client,
    variant: ResponsesVariant,
}

impl OpenAiResponsesProtocol {
    /// Create a new adapter with the given HTTP client and variant config
    pub fn new(client: Client, variant: ResponsesVariant) -> Self {
        Self { client, variant }
    }

    /// Build the endpoint URL from provider configuration and variant
    ///
    /// For Codex variant: strips trailing `/v1` only when present, appends variant path.
    /// For standard variant: normalizes base_url and appends `/v1/responses`.
    /// Default when base_url is None: `https://api.openai.com/v1/responses`
    pub fn build_endpoint(config: &ProviderConfig, variant: &ResponsesVariant) -> String {
        let endpoint_path = variant
            .endpoint_path
            .as_deref()
            .unwrap_or("/v1/responses");

        if variant.endpoint_path.is_some() {
            // Codex-style: use base_url as-is (no /v1 stripping)
            let base_url = config
                .base_url
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string())
                .unwrap_or_else(|| "https://chatgpt.com".to_string());
            format!("{}{}", base_url, endpoint_path)
        } else {
            // Standard OpenAI style: strip trailing /v1 to allow normalization
            let base_url = config
                .base_url
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let trimmed = s.trim_end_matches('/');
                    trimmed.trim_end_matches("/v1").to_string()
                })
                .unwrap_or_else(|| "https://api.openai.com".to_string());
            format!("{}{}", base_url, endpoint_path)
        }
    }

    /// Build a Responses API request from the unified payload
    ///
    /// Applies variant-specific fields (store, text, include) and auto-enables
    /// server-side optimizations for official OpenAI endpoints.
    pub fn build_responses_request(
        payload: &RequestPayload,
        model: &str,
        variant: &ResponsesVariant,
        config: &ProviderConfig,
    ) -> ResponsesRequest {
        let input = shared::convert_messages(payload.messages);
        let tools = shared::build_tools(payload.tools);
        let tool_choice = shared::map_tool_choice(payload.tool_choice.as_ref())
            .or(Some("auto".to_string()));

        // Determine store and context_management based on variant and endpoint
        let official = is_openai_official(&config.base_url);

        let (store, context_management) = if let Some(forced) = variant.store {
            // Variant explicitly sets store — respect it
            (Some(forced), None)
        } else if official {
            // Official OpenAI endpoint: enable server-side storage + compaction
            (
                Some(true),
                Some(ContextManagement {
                    mgmt_type: "compaction".into(),
                }),
            )
        } else {
            (None, None)
        };

        ResponsesRequest {
            model: model.to_string(),
            input,
            instructions: payload.system_prompt.map(|s| s.to_string()),
            stream: true,
            store,
            reasoning: shared::build_reasoning(payload.think_level),
            tools,
            tool_choice,
            parallel_tool_calls: Some(true),
            text: variant.text.clone(),
            max_output_tokens: payload.max_tokens,
            include: variant.include.clone(),
            previous_response_id: None,
            context_management,
        }
    }
}

/// Returns true when `base_url` is None (implicit OpenAI default) or points to api.openai.com
fn is_openai_official(base_url: &Option<String>) -> bool {
    match base_url {
        None => true,
        Some(url) if url.is_empty() => true,
        Some(url) => url.contains("api.openai.com"),
    }
}

#[async_trait]
impl ProtocolAdapter for OpenAiResponsesProtocol {
    fn supports_native_tools(&self) -> bool {
        true
    }

    fn supports_strict_schema(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "openai-responses"
    }

    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config, &self.variant);
        let request = Self::build_responses_request(payload, config.default_model(), &self.variant, config);

        let api_key = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("OpenAI API key not set")
        })?;

        if let Ok(json) = serde_json::to_string_pretty(&request) {
            debug!(
                endpoint = %endpoint,
                model = %config.default_model(),
                request_body = %json,
                "Building OpenAI Responses API request"
            );
        }

        let mut builder = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        // Codex-specific headers: extract account_id from JWT and set Codex mode headers
        if self.variant.endpoint_path.is_some() {
            use super::openai_common::tools::extract_codex_account_id;
            if let Some(account_id) = extract_codex_account_id(api_key) {
                builder = builder
                    .header("chatgpt-account-id", account_id)
                    .header("OpenAI-Beta", "responses=experimental")
                    .header("originator", "aleph");
            }
        }

        // Apply any variant-specific extra headers
        for (name, value) in &self.variant.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let builder = builder.json(&request);
        Ok(builder)
    }

    /// Stream fine-grained delta events from the OpenAI Responses API SSE format.
    ///
    /// Uses a pending-queue unfold approach so that the `Completed` event (which
    /// produces both a `Usage` delta and a `Done` delta) can emit both without
    /// losing either. The unfold state carries a `VecDeque<Result<ProviderDelta>>`
    /// as a lookahead buffer for multi-delta events.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
        use std::collections::VecDeque;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "OpenAI Responses API error ({}): {}",
                status, error_text
            )));
        }

        // Build a boxed stream of raw chunks with AlephError as error type
        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .boxed();

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: futures::stream::BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE line buffer
            line_buf: String,
            /// item_id → call_id mapping for tool call correlation
            item_to_call: HashMap<String, String>,
            /// Pending deltas queued from multi-delta events (e.g. Completed)
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: String::new(),
            item_to_call: HashMap::new(),
            pending: VecDeque::new(),
            done: false,
        };

        let stream = futures::stream::unfold(state, |mut state| async move {
            loop {
                // Drain pending queue first (handles multi-delta events)
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
                        parse_sse_event_multi(data, &mut state.item_to_call, &mut state.pending);
                    }
                    // Loop to drain more lines or pop from pending
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
                                parse_sse_event_multi(data, &mut state.item_to_call, &mut state.pending);
                            }
                        }
                        state.done = true;
                        // Drain any pending events queued from the flush
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
                        // Loop to try parsing again with the new data
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

/// Parse one SSE data string and push zero or more ProviderDeltas into `out`.
///
/// Uses a VecDeque to handle the `Completed` event which produces two deltas
/// (Usage + Done). The `item_to_call` map is updated in-place for tool call
/// correlation (`OutputItemAdded` records item_id → call_id).
fn parse_sse_event_multi(
    data: &str,
    item_to_call: &mut HashMap<String, String>,
    out: &mut std::collections::VecDeque<Result<ProviderDelta>>,
) {
    use crate::providers::protocols::openai_common::tools::desanitize_tool_name;
    use crate::providers::responses::types::OutputItem;

    let event = match shared::parse_sse_data(data) {
        Some(e) => e,
        None => return, // [DONE] sentinel or unparseable
    };

    match event {
        StreamEvent::TextDelta { delta, .. } => {
            out.push_back(Ok(ProviderDelta::TextDelta(delta)));
        }

        StreamEvent::OutputItemAdded {
            item: OutputItem::FunctionCall { id, call_id, name, .. },
            ..
        } => {
            // Register item_id → call_id for subsequent arg delta correlation
            item_to_call.insert(id, call_id.clone());
            out.push_back(Ok(ProviderDelta::ToolCallStart {
                id: call_id,
                name: desanitize_tool_name(&name),
            }));
        }

        StreamEvent::FunctionCallArgumentsDelta { item_id, delta, .. } => {
            if let Some(call_id) = item_to_call.get(&item_id).cloned() {
                out.push_back(Ok(ProviderDelta::ToolCallArgDelta { id: call_id, delta }));
            }
        }

        StreamEvent::FunctionCallArgumentsDone { item_id, .. } => {
            // Arguments stream complete — emit ToolCallEnd using the call_id
            if let Some(call_id) = item_to_call.get(&item_id).cloned() {
                out.push_back(Ok(ProviderDelta::ToolCallEnd { id: call_id }));
            }
        }

        StreamEvent::OutputItemDone {
            item: OutputItem::FunctionCall { call_id, .. },
            ..
        } => {
            // Fallback ToolCallEnd (some providers skip FunctionCallArgumentsDone)
            out.push_back(Ok(ProviderDelta::ToolCallEnd { id: call_id }));
        }

        StreamEvent::Completed { response } => {
            let is_incomplete = response.status == "incomplete";
            if is_incomplete {
                warn!(
                    status = %response.status,
                    "Responses API response truncated (status=incomplete)"
                );
            }

            let stop_reason = if is_incomplete {
                StopReason::MaxTokens
            } else {
                let has_tool_calls = response
                    .output
                    .iter()
                    .any(|item| matches!(item, OutputItem::FunctionCall { .. }));
                if has_tool_calls {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                }
            };

            // Emit Usage before Done so consumers can record it
            if let Some(u) = response.usage {
                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    cache_read_tokens: None,
                })));
            }
            out.push_back(Ok(ProviderDelta::Done(stop_reason)));
        }

        StreamEvent::Failed { response } => {
            let msg = response
                .error
                .map(|e| format!("{}: {}", e.code, e.message))
                .unwrap_or_else(|| "Unknown error".to_string());
            warn!(error = %msg, "Responses API stream failed");
            out.push_back(Ok(ProviderDelta::Error(msg)));
        }

        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::responses::types::{InputItem, MessageContent, StreamEvent};
    use crate::providers::responses::shared;

    // ─── Variant tests ─────────────────────────────────────────────────────

    #[test]
    fn test_codex_variant_fields() {
        let v = ResponsesVariant::codex();
        assert_eq!(v.endpoint_path.as_deref(), Some("/backend-api/codex/responses"));
        assert_eq!(v.store, Some(false));
        assert!(v.text.is_some());
        assert!(v.include.is_some());
        let include = v.include.unwrap();
        assert!(include.iter().any(|s| s == "reasoning.encrypted_content"));
    }

    #[test]
    fn test_default_variant() {
        let v = ResponsesVariant::default();
        assert!(v.endpoint_path.is_none());
        assert!(v.store.is_none());
        assert!(v.text.is_none());
        assert!(v.include.is_none());
        assert!(v.extra_headers.is_empty());
    }

    // ─── Endpoint building ────────────────────────────────────────────────

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
        assert_eq!(endpoint, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://custom.api.com/v1".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
        assert_eq!(endpoint, "https://custom.api.com/v1/responses");
    }

    #[test]
    fn test_build_endpoint_openrouter() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://openrouter.ai/api/v1".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
        assert_eq!(endpoint, "https://openrouter.ai/api/v1/responses");
    }

    #[test]
    fn test_build_endpoint_trailing_slash() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://api.example.com/v1/".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
        assert_eq!(endpoint, "https://api.example.com/v1/responses");
    }

    #[test]
    fn test_build_endpoint_codex_default() {
        let config = ProviderConfig::test_config("codex-mini-latest");
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::codex());
        assert!(endpoint.ends_with("/backend-api/codex/responses"), "got: {}", endpoint);
    }

    #[test]
    fn test_build_endpoint_codex_custom_base() {
        let mut config = ProviderConfig::test_config("codex-mini-latest");
        config.base_url = Some("https://chatgpt.com".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::codex());
        assert_eq!(endpoint, "https://chatgpt.com/backend-api/codex/responses");
    }

    // ─── Request building ─────────────────────────────────────────────────

    #[test]
    fn test_build_responses_request_basic() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let config = ProviderConfig::test_config("gpt-4o");
        let request = OpenAiResponsesProtocol::build_responses_request(
            &payload, "gpt-4o", &ResponsesVariant::default(), &config
        );

        assert_eq!(request.model, "gpt-4o");
        assert!(request.stream);
        // Official endpoint: store=true, context_management set
        assert_eq!(request.store, Some(true));
        assert!(request.context_management.is_some());
        assert!(request.text.is_none());
        assert!(request.include.is_none());
        assert!(request.instructions.is_none());
        assert!(request.reasoning.is_none());
        assert_eq!(request.input.len(), 1);
    }

    #[test]
    fn test_build_responses_request_non_official() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://openrouter.ai/api/v1".to_string());
        let request = OpenAiResponsesProtocol::build_responses_request(
            &payload, "gpt-4o", &ResponsesVariant::default(), &config
        );

        // Non-official: no store, no context_management
        assert!(request.store.is_none());
        assert!(request.context_management.is_none());
    }

    #[test]
    fn test_build_responses_request_codex() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("codex-mini-latest");
        config.base_url = Some("https://chatgpt.com".to_string());
        let request = OpenAiResponsesProtocol::build_responses_request(
            &payload, "codex-mini-latest", &ResponsesVariant::codex(), &config
        );

        assert_eq!(request.model, "codex-mini-latest");
        assert_eq!(request.store, Some(false));
        assert!(request.stream);
        assert!(request.text.is_some());
        assert!(request.include.is_some());
        assert!(request.instructions.is_none());
        assert!(request.reasoning.is_none());
        assert_eq!(request.input.len(), 1);
        match &request.input[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content.as_text(), "Hello");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_build_responses_request_with_system() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("You are helpful"));
        let config = ProviderConfig::test_config("gpt-4o");
        let request = OpenAiResponsesProtocol::build_responses_request(
            &payload, "gpt-4o", &ResponsesVariant::default(), &config
        );

        assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
    }

    #[test]
    fn test_build_responses_request_with_reasoning() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Think about this")];
        let payload = RequestPayload::new(&msgs)
            .with_think_level(Some(ThinkLevel::High));
        let config = ProviderConfig::test_config("gpt-4o");
        let request = OpenAiResponsesProtocol::build_responses_request(
            &payload, "gpt-4o", &ResponsesVariant::default(), &config
        );

        let reasoning = request.reasoning.unwrap();
        assert_eq!(reasoning.effort.as_deref(), Some("high"));
        assert_eq!(reasoning.summary.as_deref(), Some("auto"));
    }

    // ─── Adapter metadata ────────────────────────────────────────────────

    #[test]
    fn test_adapter_name() {
        let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
        assert_eq!(adapter.name(), "openai-responses");
    }

    #[test]
    fn test_supports_native_tools() {
        let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
        assert!(adapter.supports_native_tools());
    }

    // ─── is_openai_official ─────────────────────────────────────────────

    #[test]
    fn test_is_openai_official() {
        assert!(is_openai_official(&None));
        assert!(is_openai_official(&Some(String::new())));
        assert!(is_openai_official(&Some("https://api.openai.com".into())));
        assert!(is_openai_official(&Some("https://api.openai.com/v1".into())));
        assert!(!is_openai_official(&Some("https://openrouter.ai/api/v1".into())));
        assert!(!is_openai_official(&Some("https://chatgpt.com".into())));
    }

    // ─── Provider factory and preset tests (from codex.rs) ───────────────

    #[test]
    fn test_create_provider_via_factory() {
        use crate::config::ProviderConfig;
        use crate::providers::create_provider;

        let mut config = ProviderConfig::test_config("codex-mini-latest");
        config.protocol = Some("codex".to_string());
        config.api_key = Some("test_token".to_string());
        config.base_url = Some("https://chatgpt.com".to_string());
        config.enabled = true;

        let provider = create_provider("chatgpt-sub", config);
        assert!(
            provider.is_ok(),
            "Should create codex provider: {:?}",
            provider.err()
        );
    }

    #[test]
    fn test_codex_preset() {
        use crate::providers::presets::get_preset;

        let preset = get_preset("chatgpt");
        assert!(preset.is_some(), "chatgpt preset should exist");

        let p = preset.unwrap();
        assert_eq!(p.protocol, "codex");
        assert_eq!(p.default_model, "gpt-5.4");
    }

    // ─── convert_messages tests (migrated from codex.rs) ─────────────────

    #[test]
    fn test_convert_s1_pure_text_user_message() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("hello")];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: MessageContent::Text { content: "hello".into() },
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
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 3);
        match &items[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content.as_text(), "What is Rust?");
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
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 2);
        match &items[1] {
            InputItem::FunctionCall { call_id, name, .. } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(name, "web_search");
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
        let items = shared::convert_messages(&msgs);

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
        let items = shared::convert_messages(&msgs);

        // User(1) + FunctionCall(1) + FunctionCallOutput(1) + Assistant Message(1) = 4
        assert_eq!(items.len(), 4);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: MessageContent::Text { content: "Search for Rust tutorials".into() },
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
        let items = shared::convert_messages(&msgs);

        // 1 Message (text) + 3 FunctionCalls = 4
        assert_eq!(items.len(), 4);
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
            true,
        )];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
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
        let items = shared::convert_messages(&msgs);

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
        let data = r#"{"type":"response.completed","response":{"id":"resp_u","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
        let event = shared::parse_sse_data(data).unwrap();
        match event {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                assert_eq!(
                    shared::extract_text(&response),
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
        let data = r#"{"type":"response.completed","response":{"id":"resp_inc","status":"incomplete","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"partial"}]}]}}"#;
        let event = shared::parse_sse_data(data).unwrap();
        match event {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "incomplete");
                assert_eq!(
                    shared::extract_text(&response),
                    Some("partial".to_string())
                );
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    // ─── parse_sse_data tests (shared, migrated from codex.rs) ────────────

    #[test]
    fn test_parse_sse_data_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
        let event = shared::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_done() {
        let result = shared::parse_sse_data("[DONE]");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_completed() {
        let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
        let event = shared::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                let text = shared::extract_text(&response);
                assert_eq!(text, Some("Hello world".to_string()));
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_text_from_response() {
        use crate::providers::responses::types::{ContentPart, OutputItem, ResponseResource};
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![OutputItem::Message {
                id: "msg_1".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentPart {
                    part_type: "output_text".to_string(),
                    text: "Test output".to_string(),
                }],
            }],
            usage: None,
            error: None,
        };
        assert_eq!(
            shared::extract_text(&response),
            Some("Test output".to_string())
        );
    }

    #[test]
    fn test_extract_text_empty_output() {
        use crate::providers::responses::types::ResponseResource;
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![],
            usage: None,
            error: None,
        };
        assert_eq!(shared::extract_text(&response), None);
    }

    // ─── parse_sse_event_multi unit tests ────────────────────────────────

    fn drain_one(
        data: &str,
        map: &mut HashMap<String, String>,
    ) -> Option<ProviderDelta> {
        let mut out = std::collections::VecDeque::new();
        parse_sse_event_multi(data, map, &mut out);
        out.pop_front().and_then(|r| r.ok())
    }

    fn drain_all(
        data: &str,
        map: &mut HashMap<String, String>,
    ) -> Vec<ProviderDelta> {
        let mut out = std::collections::VecDeque::new();
        parse_sse_event_multi(data, map, &mut out);
        out.into_iter().filter_map(|r| r.ok()).collect()
    }

    #[test]
    fn test_parse_sse_event_text_delta() {
        let mut map = HashMap::new();
        let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
        let delta = drain_one(data, &mut map);
        assert!(matches!(delta, Some(ProviderDelta::TextDelta(ref s)) if s == "Hello"));
    }

    #[test]
    fn test_parse_sse_event_tool_call_start() {
        let mut map = HashMap::new();
        let data = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"search","arguments":""}}"#;
        let delta = drain_one(data, &mut map);
        assert!(matches!(delta, Some(ProviderDelta::ToolCallStart { ref id, ref name }) if id == "call_abc" && name == "search"));
        // item_id → call_id mapping populated
        assert_eq!(map.get("fc_1").map(|s| s.as_str()), Some("call_abc"));
    }

    #[test]
    fn test_parse_sse_event_arg_delta_requires_mapping() {
        let mut map = HashMap::new();
        // Without the mapping, arg delta produces no output
        let data = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"q\":"}"#;
        let delta = drain_one(data, &mut map);
        assert!(delta.is_none(), "Should produce nothing when item_id not mapped");

        // Register mapping and try again
        map.insert("fc_1".to_string(), "call_abc".to_string());
        let delta2 = drain_one(data, &mut map);
        assert!(matches!(delta2, Some(ProviderDelta::ToolCallArgDelta { ref id, .. }) if id == "call_abc"));
    }

    #[test]
    fn test_parse_sse_event_args_done() {
        let mut map = HashMap::new();
        map.insert("fc_1".to_string(), "call_abc".to_string());
        let data = r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","arguments":"{\"q\":\"rust\"}"}"#;
        let delta = drain_one(data, &mut map);
        assert!(matches!(delta, Some(ProviderDelta::ToolCallEnd { ref id }) if id == "call_abc"));
    }

    #[test]
    fn test_parse_sse_event_completed_emits_usage_and_done() {
        let mut map = HashMap::new();
        let data = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"test","output":[{"type":"message","id":"m1","role":"assistant","content":[{"type":"output_text","text":"hi"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
        let deltas = drain_all(data, &mut map);
        assert_eq!(deltas.len(), 2, "Completed should emit Usage + Done");
        assert!(matches!(&deltas[0], ProviderDelta::Usage(u) if u.input_tokens == 10 && u.output_tokens == 5));
        assert!(matches!(&deltas[1], ProviderDelta::Done(StopReason::EndTurn)));
    }

    #[test]
    fn test_parse_sse_event_completed_no_usage_emits_done_only() {
        let mut map = HashMap::new();
        let data = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"test","output":[]}}"#;
        let deltas = drain_all(data, &mut map);
        assert_eq!(deltas.len(), 1, "Completed with no usage should emit only Done");
        assert!(matches!(&deltas[0], ProviderDelta::Done(StopReason::EndTurn)));
    }

    #[test]
    fn test_parse_sse_event_incomplete_emits_max_tokens() {
        let mut map = HashMap::new();
        let data = r#"{"type":"response.completed","response":{"id":"r1","status":"incomplete","model":"test","output":[]}}"#;
        let deltas = drain_all(data, &mut map);
        assert!(!deltas.is_empty());
        let done = deltas.last().unwrap();
        assert!(matches!(done, ProviderDelta::Done(StopReason::MaxTokens)));
    }
}
