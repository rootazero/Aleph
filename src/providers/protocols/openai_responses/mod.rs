//! `OpenAI` Responses API protocol adapter
//!
//! Handles both the standard `OpenAI` Responses API at /v1/responses and the
//! Codex variant at /backend-api/codex/responses via `ResponsesVariant`.

use std::collections::HashMap;

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason, TokenUsage};
use crate::providers::delta::ProviderDelta;
use crate::providers::protocols::openai_common::provider_policy::build_payload_policy;
use crate::providers::protocols::openai_common::response_format::merge_text_format;
use crate::providers::responses::shared;
use crate::providers::responses::types::{
    ContextManagement, ResponsesRequest, StreamEvent, TextConfig,
};
use crate::sync_primitives::Arc;
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
/// Allows the same adapter to be used for standard `OpenAI` (`Default`) and
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
    /// (store=false, verbosity=medium, `reasoning.encrypted_content` include).
    #[must_use]
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

/// `OpenAI` Responses API protocol adapter
///
/// Translates between Aleph's unified request format and the `OpenAI` Responses
/// API wire format. Supports both standard `OpenAI` and Codex endpoints via
/// `ResponsesVariant`.
pub struct OpenAiResponsesProtocol {
    client: Client,
    variant: ResponsesVariant,
    /// Idle timeout (seconds) for the SSE byte stream, resolved from
    /// `ProviderConfig.stream_idle_timeout_secs` in `build_request` and read
    /// in `stream_deltas`. An `AtomicU64` because `&self` is shared (`Arc`)
    /// and the value must cross into the `'static` stream closure.
    stream_idle_timeout_secs: Arc<crate::sync_primitives::AtomicU64>,
}

impl OpenAiResponsesProtocol {
    /// Create a new adapter with the given HTTP client and variant config
    #[must_use]
    pub fn new(client: Client, variant: ResponsesVariant) -> Self {
        Self {
            client,
            variant,
            stream_idle_timeout_secs: Arc::new(crate::sync_primitives::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }

    /// Build the endpoint URL from provider configuration and variant
    ///
    /// For Codex variant: strips trailing `/v1` only when present, appends variant path.
    /// For standard variant: normalizes `base_url` and appends `/v1/responses`.
    /// Default when `base_url` is None: `https://api.openai.com/v1/responses`
    #[must_use]
    pub fn build_endpoint(config: &ProviderConfig, variant: &ResponsesVariant) -> String {
        let endpoint_path = variant.endpoint_path.as_deref().unwrap_or("/v1/responses");

        if variant.endpoint_path.is_some() {
            // Codex-style: use base_url as-is (no /v1 stripping)
            let base_url = config
                .base_url
                .as_ref()
                .filter(|s| !s.is_empty())
                .map_or_else(
                    || "https://chatgpt.com".to_string(),
                    |s| s.trim_end_matches('/').to_string(),
                );
            format!("{base_url}{endpoint_path}")
        } else {
            // Standard OpenAI style: strip trailing /v1 to allow normalization
            let base_url = config
                .base_url
                .as_ref()
                .filter(|s| !s.is_empty())
                .map_or_else(
                    || "https://api.openai.com".to_string(),
                    |s| {
                        let trimmed = s.trim_end_matches('/');
                        trimmed.trim_end_matches("/v1").to_string()
                    },
                );
            format!("{base_url}{endpoint_path}")
        }
    }

    /// Build a Responses API request from the unified payload
    ///
    /// Applies variant-specific fields (store, text, include) and auto-enables
    /// server-side optimizations for official `OpenAI` endpoints.
    pub fn build_responses_request(
        payload: &RequestPayload,
        model: &str,
        variant: &ResponsesVariant,
        config: &ProviderConfig,
    ) -> ResponsesRequest {
        let input = shared::convert_messages(payload.messages);
        let policy = build_payload_policy(
            config.base_url.as_deref(),
            "openai-responses",
            variant.store,
        );
        let tools = shared::build_tools(payload.tools, policy.capabilities.supports_strict_schema);
        let tool_choice = shared::map_tool_choice(payload.tool_choice.as_ref())
            .or_else(|| Some(serde_json::Value::from("auto")));

        let store = policy.explicit_store;

        let context_mgmt = if policy.capabilities.supports_server_compaction {
            Some(ContextManagement {
                mgmt_type: "compaction".into(),
            })
        } else {
            None
        };

        let stop = config.stop_sequences.as_ref().and_then(|raw| {
            let v: Vec<String> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        });

        ResponsesRequest {
            model: model.to_string(),
            input,
            instructions: payload.system_prompt.map(|s| s.to_string()),
            stream: true,
            store,
            reasoning: shared::build_reasoning(payload.think_level, model),
            tools,
            tool_choice,
            parallel_tool_calls: config.parallel_tool_calls,
            text: merge_text_format(
                // rust-doctor-disable-next-line excessive-clone
                variant.text.clone(),
                config.response_format.as_ref(),
                policy.capabilities.supports_strict_schema,
            ),
            max_output_tokens: payload.max_tokens,
            // rust-doctor-disable-next-line excessive-clone
            include: variant.include.clone().or_else(|| {
                if policy.endpoint_class
                    == crate::providers::protocols::openai_common::provider_policy::EndpointClass::OpenAiPublic
                {
                    Some(vec!["reasoning.encrypted_content".into()])
                } else {
                    None
                }
            }),
            previous_response_id: None,
            context_management: context_mgmt,
            stop,
            seed: config.seed.filter(|_| policy.capabilities.supports_seed),
            top_logprobs: if config.logprobs == Some(true)
                && policy.capabilities.supports_logprobs
            {
                // The Responses API has no `logprobs: bool` toggle — `top_logprobs`
                // is the only opt-in signal (it is not added to `include`). When the
                // user opted in (logprobs=true) but gave no count, emit 0 (the chosen
                // token's logprob, no alternatives) rather than omitting the field,
                // which would drop the opt-in entirely.
                Some(config.top_logprobs.map_or(0, u32::from))
            } else {
                None
            },
            service_tier: config
                .service_tier
                // rust-doctor-disable-next-line excessive-clone
                .clone()
                .filter(|_| policy.capabilities.supports_service_tier),
            // Content-addressed routing affinity (static prefix hash, session
            // id fallback) on endpoints that honor it; omitted elsewhere (and
            // stripped defensively by policy).
            prompt_cache_key: if policy.capabilities.supports_prompt_cache {
                crate::providers::protocols::openai_common::prompt_cache::derive_prompt_cache_key(
                    payload,
                )
            } else {
                None
            },
            // Extended cache retention (24h) — official OpenAI only; the
            // user's `cache_retention = long` knob maps to it (the Anthropic
            // side maps the same knob to the 1h ephemeral TTL).
            prompt_cache_retention: if policy.capabilities.supports_prompt_cache
                && matches!(
                    config.cache_retention,
                    Some(crate::config::types::provider::CacheRetention::Long)
                )
                && policy.endpoint_class
                    == crate::providers::protocols::openai_common::provider_policy::EndpointClass::OpenAiPublic
            {
                Some("24h".to_string())
            } else {
                None
            },
        }
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

    /// Share the Chat protocol's canonicalization (no-dash typos, `openai/`
    /// prefix). Endpoint-blind hook (assumes first-party `OpenAI`); `build_request`
    /// calls [`normalize_openai_model_id`] directly with the configured
    /// `base_url` so the `openai/` slug survives on aggregators (`OpenRouter`,
    /// which ships an `openai-responses` preset).
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        super::openai_common::model_id::normalize_openai_model_id(model_id, None)
    }

    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        self.stream_idle_timeout_secs.store(
            crate::providers::protocols::stream_idle::effective_idle_secs(config),
            std::sync::atomic::Ordering::Relaxed,
        );
        let endpoint = Self::build_endpoint(config, &self.variant);
        let raw_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        // Endpoint-aware: keep the `openai/` slug on aggregators (OpenRouter),
        // strip it on the first-party OpenAI Responses API. See `model_id`.
        let actual_model = super::openai_common::model_id::normalize_openai_model_id(
            raw_model,
            config.base_url.as_deref(),
        );
        let request = Self::build_responses_request(payload, &actual_model, &self.variant, config);

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
            .header("Authorization", format!("Bearer {api_key}"))
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
            // Session affinity: the reference Codex CLI always sends `session-id`
            // so the ChatGPT backend can bind prompt-cache / rate-limit state to
            // the agent session. Sourced from the same `metadata["session_id"]`
            // that seeds `prompt_cache_key` above.
            if let Some(session_id) = payload
                .metadata
                .as_ref()
                .and_then(|m| m.get("session_id"))
                .filter(|s| !s.is_empty())
            {
                builder = builder.header("session-id", session_id.as_str());
            }
        }

        // Apply any variant-specific extra headers
        for (name, value) in &self.variant.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let builder = builder.json(&request);
        Ok(builder)
    }

    /// Stream fine-grained delta events from the `OpenAI` Responses API SSE format.
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
            let retry_after =
                crate::providers::protocols::http_client::retry_after_secs(response.headers());
            let error_text =
                crate::providers::protocols::http_client::read_error_body(response).await;
            if status.as_u16() == 429 {
                let suggestion = retry_after.as_ref().map_or_else(
                    || "Rate limited. Wait before retrying or upgrade your API plan.".to_string(),
                    |ra| format!("Rate limited. Retry after {ra} seconds."),
                );
                return Err(AlephError::RateLimitError {
                    message: format!("OpenAI Responses API rate limited (429): {error_text}"),
                    suggestion: Some(suggestion),
                });
            }
            // Mirror the Chat adapter: some OpenAI-compatible providers signal
            // quota/spending exhaustion with a non-429 status and a descriptive
            // body (xAI 403, OpenAI `insufficient_quota`). Surface those as a
            // typed usage-limit error instead of a generic provider fault.
            if crate::providers::protocols::openai_common::usage_limit::is_usage_limit_body(
                &error_text,
            ) {
                return Err(AlephError::RateLimitError {
                    message: format!("Provider usage limit reached ({status}): {error_text}"),
                    suggestion: Some(
                        "Account quota or spending limit exhausted. Upgrade your plan or wait \
                         for the quota to reset — retrying will not help."
                            .to_string(),
                    ),
                });
            }
            return Err(AlephError::provider(format!(
                "OpenAI Responses API error ({status}): {error_text}"
            )));
        }

        // Build a boxed stream of raw chunks with AlephError as error type
        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {e}")))
            .boxed();
        let idle_secs = self
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        let byte_stream = crate::providers::protocols::stream_idle::wrap_idle_timeout(
            byte_stream,
            idle_secs,
            "OpenAI",
        );

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: futures::stream::BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE byte buffer (bytes, not String — HTTP chunks may
            /// split multi-byte UTF-8 characters at arbitrary boundaries)
            line_buf: Vec<u8>,
            /// `item_id` → `call_id` mapping for tool call correlation
            item_to_call: HashMap<String, String>,
            /// Pending deltas queued from multi-delta events (e.g. Completed)
            pending: VecDeque<Result<ProviderDelta>>,
            /// True once a terminal delta (`Done` from `response.completed`,
            /// `Error` from `response.failed` / a top-level error frame) has
            /// been queued. Unlike `done`, this must survive across
            /// iterations: after `Completed` the stream keeps reading until
            /// the `[DONE]` sentinel and natural HTTP end, so the end-of-bytes
            /// branch needs to know a terminal signal was already seen.
            saw_terminal: bool,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
            item_to_call: HashMap::new(),
            pending: VecDeque::new(),
            saw_terminal: false,
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
                        if crate::providers::delta::has_terminal_delta(&state.pending) {
                            state.saw_terminal = true;
                        }
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
                                if crate::providers::delta::has_terminal_delta(&state.pending) {
                                    state.saw_terminal = true;
                                }
                            }
                        }
                        // No `response.completed` / `response.failed` / error
                        // frame across the whole stream means the connection
                        // dropped mid-response. Surface a typed transient error
                        // instead of letting the collector default to `EndTurn`
                        // and present truncated output as a complete turn.
                        if !state.saw_terminal {
                            state.pending.push_back(Err(AlephError::Timeout {
                                suggestion: Some(
                                    "OpenAI Responses stream closed before \
                                     response.completed arrived — the response was \
                                     truncated mid-stream. Retry or switch providers."
                                        .to_string(),
                                ),
                            }));
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

/// Parse one SSE data string and push zero or more `ProviderDeltas` into `out`.
///
/// Uses a `VecDeque` to handle the `Completed` event which produces two deltas
/// (Usage + Done). The `item_to_call` map is updated in-place for tool call
/// correlation (`OutputItemAdded` records `item_id` → `call_id`).
// rust-doctor-disable-next-line high-cyclomatic-complexity
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
        StreamEvent::TextDone { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.output_text.done — content already accumulated via delta events"
            );
        }

        StreamEvent::OutputItemAdded {
            item: OutputItem::FunctionCall {
                id, call_id, name, ..
            },
            ..
        } => {
            // Register item_id → call_id for subsequent arg delta correlation
            // rust-doctor-disable-next-line excessive-clone
            item_to_call.insert(id, call_id.clone());
            out.push_back(Ok(ProviderDelta::ToolCallStart {
                signature: None,
                id: call_id,
                name: desanitize_tool_name(&name),
            }));
        }

        StreamEvent::FunctionCallArgumentsDelta { item_id, delta, .. } => {
            if let Some(call_id) = item_to_call.get(&item_id).cloned() {
                out.push_back(Ok(ProviderDelta::ToolCallArgDelta { id: call_id, delta }));
            }
        }

        StreamEvent::FunctionCallArgumentsDone {
            item_id, arguments, ..
        } => {
            // Arguments stream complete. `arguments` is the fully-assembled,
            // authoritative copy — the streamed `...arguments.delta` fragments
            // can arrive truncated (large values dropped mid-stream), so adopt
            // this copy before ending the call.
            if let Some(call_id) = item_to_call.get(&item_id).cloned() {
                out.push_back(Ok(ProviderDelta::ToolCallArgsComplete {
                    // rust-doctor-disable-next-line excessive-clone
                    id: call_id.clone(),
                    arguments,
                }));
                out.push_back(Ok(ProviderDelta::ToolCallEnd { id: call_id }));
            }
        }

        StreamEvent::OutputItemDone {
            item: OutputItem::FunctionCall {
                call_id, arguments, ..
            },
            ..
        } => {
            // Fallback for backends that skip FunctionCallArgumentsDone: the
            // final output item also carries the complete arguments. Adopt them
            // (no-op when empty) so a truncated delta stream is repaired.
            out.push_back(Ok(ProviderDelta::ToolCallArgsComplete {
                // rust-doctor-disable-next-line excessive-clone
                id: call_id.clone(),
                arguments,
            }));
            out.push_back(Ok(ProviderDelta::ToolCallEnd { id: call_id }));
        }

        StreamEvent::OutputItemDone {
            item:
                OutputItem::Reasoning {
                    id,
                    encrypted_content: Some(ec),
                    ..
                },
            ..
        } => {
            // Persist the encrypted reasoning blob for cross-turn replay
            // (stateless `store:false` flows). NDJSON-encoded into the thinking
            // signature: the DeltaCollector concatenates these fragments, and
            // `convert_messages` parses each line back into a reasoning input
            // item on the next turn.
            if let Ok(line) = serde_json::to_string(&serde_json::json!({"id": id, "ec": ec})) {
                out.push_back(Ok(ProviderDelta::ThinkingSignatureDelta(format!(
                    "{line}\n"
                ))));
            }
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
                // A tool call counts whether it appears in the final `output`
                // array OR was streamed during the run. The Codex
                // (chatgpt.com) backend can return an empty `output` on
                // `response.completed` even after emitting function-call
                // events; `item_to_call` is the authoritative streamed record,
                // so without this check the turn is mis-classified EndTurn and
                // the streamed tool calls are never executed.
                let has_tool_calls = !item_to_call.is_empty()
                    || response
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
                let cache_read_tokens = u
                    .input_tokens_details
                    .as_ref()
                    .and_then(|d| d.cached_tokens);
                let thinking_tokens = u
                    .output_tokens_details
                    .as_ref()
                    .and_then(|d| d.reasoning_tokens);

                // `input_tokens` *includes* `input_tokens_details.cached_tokens`
                // on this protocol, while Aleph's pricing bills `input` and
                // `cache_read` additively (disjoint, Anthropic-shaped). Report
                // the non-cached remainder as input — otherwise every cache hit
                // is billed twice, and the error grows with cache
                // effectiveness. Same subtraction as the Gemini adapter.
                let input_tokens = u
                    .input_tokens
                    .saturating_sub(cache_read_tokens.unwrap_or(0));

                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens,
                    output_tokens: u.output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens: None, // Responses API does not surface cache-write
                    thinking_tokens,
                    cost: None,
                })));
            }
            out.push_back(Ok(ProviderDelta::Done(stop_reason)));
        }

        StreamEvent::Failed { response } => {
            let msg = response.error.map_or_else(
                || "Unknown error".to_string(),
                |e| format!("{}: {}", e.code, e.message),
            );
            warn!(error = %msg, "Responses API stream failed");
            out.push_back(Ok(ProviderDelta::Error(msg)));
        }

        StreamEvent::Error {
            code,
            message,
            param,
        } => {
            // Top-level error frame (not wrapped in a `response` object).
            let detail = match (code, param) {
                (Some(c), Some(p)) => format!("{c} (param={p}): {message}"),
                (Some(c), None) => format!("{c}: {message}"),
                (None, Some(p)) => format!("(param={p}): {message}"),
                (None, None) => message,
            };
            warn!(error = %detail, "Responses API stream error frame");
            out.push_back(Ok(ProviderDelta::Error(detail)));
        }

        StreamEvent::ReasoningSummaryPartAdded { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "reasoning_summary_part.added — boundary marker, no canonical delta emitted"
            );
        }
        StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
            out.push_back(Ok(ProviderDelta::ThinkingDelta(delta)));
        }
        StreamEvent::ReasoningSummaryTextDone { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "reasoning_summary_text.done — content already accumulated via delta events"
            );
        }
        StreamEvent::ReasoningSummaryPartDone { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "reasoning_summary_part.done — boundary marker, no canonical delta emitted"
            );
        }

        StreamEvent::ReasoningTextDelta { delta, .. } => {
            // Raw (unsummarized) chain-of-thought — same canonical delta as
            // the summary variant so consumers render it uniformly.
            out.push_back(Ok(ProviderDelta::ThinkingDelta(delta)));
        }
        StreamEvent::ReasoningTextDone { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "reasoning_text.done — content already accumulated via delta events"
            );
        }

        StreamEvent::Created { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.created — lifecycle event, no canonical delta emitted"
            );
        }
        StreamEvent::InProgress { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.in_progress — lifecycle event, no canonical delta emitted"
            );
        }
        StreamEvent::ContentPartAdded { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.content_part.added — boundary marker, no canonical delta emitted"
            );
        }
        StreamEvent::ContentPartDone { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.content_part.done — boundary marker, no canonical delta emitted"
            );
        }
        StreamEvent::OutputItemAdded { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.output_item.added — non-tool item, no canonical delta emitted"
            );
        }
        StreamEvent::OutputItemDone { .. } => {
            tracing::debug!(
                target: "aleph::openai_responses_sse",
                "response.output_item.done — non-tool item, no canonical delta emitted"
            );
        }
    }
}

#[cfg(test)]
mod tests;
