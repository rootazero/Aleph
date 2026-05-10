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
use crate::providers::responses::types::{
    ContextManagement, ResponsesRequest, StreamEvent, TextConfig,
};
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
        let endpoint_path = variant.endpoint_path.as_deref().unwrap_or("/v1/responses");

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
        let tool_choice =
            shared::map_tool_choice(payload.tool_choice.as_ref()).or(Some("auto".to_string()));

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
            include: variant.include.clone().or_else(|| {
                if official {
                    Some(vec!["reasoning.encrypted_content".into()])
                } else {
                    None
                }
            }),
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
        let actual_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        let request = Self::build_responses_request(payload, actual_model, &self.variant, config);

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("OpenAI API key not set"))?;

        if let Ok(json) = serde_json::to_string_pretty(&request) {
            debug!(
                endpoint = %endpoint,
                model = %actual_model,
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
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let error_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                let suggestion = retry_after
                    .as_ref()
                    .map(|ra| format!("Rate limited. Retry after {ra} seconds."))
                    .unwrap_or_else(|| {
                        "Rate limited. Wait before retrying or upgrade your API plan.".to_string()
                    });
                return Err(AlephError::RateLimitError {
                    message: format!("OpenAI Responses API rate limited (429): {}", error_text),
                    suggestion: Some(suggestion),
                });
            }
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
            /// Incomplete SSE byte buffer (bytes, not String — HTTP chunks may
            /// split multi-byte UTF-8 characters at arbitrary boundaries)
            line_buf: Vec<u8>,
            /// item_id → call_id mapping for tool call correlation
            item_to_call: HashMap<String, String>,
            /// Pending deltas queued from multi-delta events (e.g. Completed)
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
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

                // Try to parse a complete SSE line from the byte buffer.
                // Complete lines (up to \n) are safe to decode as UTF-8 since
                // SSE data lines are always complete JSON terminated by newline.
                if let Some(pos) = state.line_buf.iter().position(|&b| b == b'\n') {
                    let line_bytes = state.line_buf[..pos].to_vec();
                    state.line_buf.drain(..=pos);

                    let line = String::from_utf8(line_bytes)
                        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                    let line = line.trim_end();

                    if let Some(data) = line
                        .strip_prefix("data: ")
                        .or_else(|| line.strip_prefix("data:"))
                    {
                        parse_sse_event_multi(data, &mut state.item_to_call, &mut state.pending);
                    }
                    // Loop to drain more lines or pop from pending
                    continue;
                }

                // No complete line — fetch next chunk from HTTP
                match state.bytes.next().await {
                    None => {
                        // HTTP stream ended — flush any remaining partial line
                        let remaining = String::from_utf8(std::mem::take(&mut state.line_buf))
                            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                        let remaining = remaining.trim();
                        if !remaining.is_empty() {
                            if let Some(data) = remaining
                                .strip_prefix("data: ")
                                .or_else(|| remaining.strip_prefix("data:"))
                            {
                                parse_sse_event_multi(
                                    data,
                                    &mut state.item_to_call,
                                    &mut state.pending,
                                );
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
                        // Append raw bytes — no UTF-8 conversion here.
                        // Conversion happens per-line when a \n is found.
                        state.line_buf.extend_from_slice(&chunk);
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
            item: OutputItem::FunctionCall {
                id, call_id, name, ..
            },
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
                    cache_creation_tokens: None,
                    thinking_tokens: None,
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

        StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
            out.push_back(Ok(ProviderDelta::ThinkingDelta(delta)));
        }

        _ => {}
    }
}

#[cfg(test)]
mod tests;

