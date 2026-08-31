//! `ProtocolAdapter` trait implementation for `OpenAiProtocol`.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, ToolChoice};
use crate::providers::delta::ProviderDelta;
use crate::providers::openai::{OpenAiFunction, OpenAiTool};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt as _;
use futures::TryStreamExt;
use serde_json::json;
use std::collections::VecDeque;
use tracing::debug;

use super::sse::parse_chat_sse_event;
use super::{sanitize_tool_name, OpenAiProtocol};

use crate::providers::protocols::openai_common::max_tokens::uses_max_completion_tokens;
use crate::providers::protocols::openai_common::openai_strict_schema::{
    ensure_openai_tool_envelope, lenient_multi_type_rewrite, normalize_strict_schema, StrictResult,
};
use crate::providers::protocols::openai_common::provider_policy::build_payload_policy;
use crate::providers::protocols::openai_common::response_format::to_chat_response_format;
use crate::providers::protocols::openai_common::usage_limit::is_usage_limit_body;

#[async_trait]
impl ProtocolAdapter for OpenAiProtocol {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        self.stream_idle_timeout_secs.store(
            crate::providers::protocols::stream_idle::effective_idle_secs(config),
            std::sync::atomic::Ordering::Relaxed,
        );
        let endpoint = Self::build_endpoint(config);
        let messages = Self::convert_messages(payload.messages, payload.system_prompt);
        let raw_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        // Endpoint-aware: preserves the `openai/` slug on aggregators (OpenRouter)
        // while stripping it on the first-party OpenAI API. See `model_id`.
        let model_name =
            crate::providers::protocols::openai_common::model_id::normalize_openai_model_id(
                raw_model,
                config.base_url.as_deref(),
            )
            .into_owned();

        // Build request body — always streaming (stream-first architecture).
        // `stream_options.include_usage` makes OpenAI emit a trailing chunk
        // carrying token counts; without it the Chat Completions stream omits
        // usage entirely, leaving cost metering and context budgeting blind.
        let mut body = json!({
            "model": model_name,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        // Add optional parameters (per-request overrides provider config)
        if let Some(max_tokens) = payload.max_tokens.or(config.max_tokens) {
            let field = if uses_max_completion_tokens(&model_name) {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body[field] = json!(max_tokens);
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

        // Add reasoning_effort for thinking models, clamped to the values the
        // target model's family actually accepts (an unsupported effort like
        // `minimal` on a generic reasoning model is a hard 400).
        if let Some(ref level) = payload.think_level {
            if let Some(effort) = Self::map_think_level(level) {
                if let Some(clamped) =
                    crate::providers::protocols::openai_common::reasoning_effort::clamp_effort(
                        &model_name,
                        &effort,
                    )
                {
                    body["reasoning_effort"] = json!(clamped);
                }
            }
        }

        // stop sequences: parse comma-separated config value, trim, drop empties
        if let Some(raw) = config.stop_sequences.as_ref() {
            let sequences: Vec<String> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !sequences.is_empty() {
                body["stop"] = json!(sequences);
            }
        }

        let policy = build_payload_policy(config.base_url.as_deref(), "openai-chat", None);

        // prompt_cache_key: content-addressed routing affinity (static prefix
        // hash, session-id fallback) when the endpoint honors it. `policy.apply`
        // strips it on unsupported endpoints, but gate here too so it is never
        // set needlessly.
        if policy.capabilities.supports_prompt_cache {
            if let Some(key) =
                super::super::openai_common::prompt_cache::derive_prompt_cache_key(payload)
            {
                body["prompt_cache_key"] = json!(key);
            }
            // Extended cache retention (24h) — official OpenAI only; the
            // user's `cache_retention = long` knob maps to it (the Anthropic
            // side maps the same knob to the 1h ephemeral TTL).
            if matches!(
                config.cache_retention,
                Some(crate::config::types::provider::CacheRetention::Long)
            ) && policy.endpoint_class
                == super::super::openai_common::provider_policy::EndpointClass::OpenAiPublic
            {
                body["prompt_cache_retention"] = json!("24h");
            }
        }

        // response_format: emit only when capability-enabled
        if let Some(ref fmt) = config.response_format {
            if policy.capabilities.supports_response_format {
                if let Some(v) =
                    to_chat_response_format(fmt, policy.capabilities.supports_strict_schema)
                {
                    body["response_format"] = v;
                }
            }
        }

        // seed: emit only when capability-enabled
        if let Some(seed) = config.seed {
            if policy.capabilities.supports_seed {
                body["seed"] = json!(seed);
            }
        }

        // logprobs + top_logprobs: emit only when capability-enabled
        if let Some(want_logprobs) = config.logprobs {
            if policy.capabilities.supports_logprobs {
                body["logprobs"] = json!(want_logprobs);
                if want_logprobs {
                    if let Some(top_n) = config.top_logprobs {
                        body["top_logprobs"] = json!(top_n);
                    }
                }
            }
        }

        if let Some(tool_defs) = payload.tools {
            let tools: Vec<OpenAiTool> = tool_defs
                .iter()
                .map(|td| {
                    // rust-doctor-disable-next-line excessive-clone
                    let mut params = td.parameters.clone();
                    // Honor the strict-normalization verdict: an `Incompatible`
                    // schema (e.g. a multi-type field) cannot be shipped with
                    // `strict: true` — doing so 400s the whole request. Downgrade
                    // that single tool to non-strict and apply the lenient rewrite
                    // instead, mirroring the Responses protocol path.
                    let strict = if policy.capabilities.supports_strict_schema {
                        match normalize_strict_schema(&mut params, true) {
                            StrictResult::Ok => Some(true),
                            StrictResult::Incompatible { reason } => {
                                debug!(
                                    tool_name = %td.name,
                                    reason = %reason,
                                    "OpenAI strict mode incompatible — downgrading this tool to non-strict",
                                );
                                // rust-doctor-disable-next-line excessive-clone
                                params = td.parameters.clone();
                                lenient_multi_type_rewrite(&mut params);
                                None
                            }
                        }
                    } else {
                        None
                    };
                    // Ensure the tool envelope is always valid: OpenAI's parser
                    // requires top-level `type: "object"` and rejects `oneOf`/
                    // `anyOf` at the root. Do this before provider-specific fixes
                    // so Moonshot/DeepSeek/etc. see a well-formed object schema.
                    ensure_openai_tool_envelope(&mut params);

                    // Provider-specific schema fixes (e.g. Moonshot cannot handle
                    // local `$ref` nodes and requires explicit `type` on every
                    // property). Applied after strict/lenient normalization so the
                    // schema is already in its final shape.
                    policy.apply_to_schema(&mut params);
                    OpenAiTool {
                        tool_type: "function".into(),
                        function: OpenAiFunction {
                            name: sanitize_tool_name(&td.name),
                            // rust-doctor-disable-next-line excessive-clone
                            description: td.description.clone(),
                            parameters: params,
                            strict,
                        },
                    }
                })
                .collect();
            body["tools"] = serde_json::to_value(&tools)
                .map_err(|e| AlephError::provider(format!("Failed to serialize tools: {e}")))?;
        }

        if let Some(obj) = body.as_object_mut() {
            policy.apply(obj);
        }

        // Add tool_choice if specified
        if let Some(ref choice) = payload.tool_choice {
            body["tool_choice"] = match choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::Required => json!("required"),
                ToolChoice::Specific(name) => {
                    json!({"type": "function", "function": {"name": name}})
                }
                ToolChoice::None => json!("none"),
            };
        }

        // parallel_tool_calls: emit only when config explicitly sets it
        if let Some(parallel) = config.parallel_tool_calls {
            body["parallel_tool_calls"] = json!(parallel);
        }

        // service_tier: opt-in latency/cost tier ("auto" | "default" | "flex" |
        // "priority"). Capability-gated inline so only endpoints that accept it
        // (official OpenAI) receive the field; third-party OpenAI-compatible
        // backends never see it. The config field is shared with the Anthropic
        // adapter, which already wires it — this closes the OpenAI dead wire.
        if let Some(ref tier) = config.service_tier {
            if policy.capabilities.supports_service_tier {
                body["service_tier"] = json!(tier);
            }
        }

        // Validate API key
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %model_name,
            "Building OpenAI request"
        );

        Ok(self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body))
    }

    /// Stream fine-grained delta events from the `OpenAI` Chat Completions SSE format.
    ///
    /// Parses SSE events from the Chat Completions streaming format and emits
    /// fine-grained [`ProviderDelta`] events. Uses the unfold+pending-queue pattern
    /// so that `finish_reason` chunks (which produce multiple deltas) can emit all
    /// of them without loss.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
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
                    message: format!("OpenAI Chat API rate limited (429): {error_text}"),
                    suggestion: Some(suggestion),
                });
            }
            // Some OpenAI-compatible providers signal quota/spending exhaustion
            // with a non-429 status and a descriptive body rather than 429 —
            // notably xAI (Grok) returns HTTP 403 "used all available credits or
            // reached its monthly spending limit". Surface these as a typed
            // (non-retryable) usage-limit error instead of swallowing them into a
            // generic provider error that reads as a transient fault. (#86614)
            if is_usage_limit_body(&error_text) {
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
                "OpenAI Chat API error ({status}): {error_text}"
            )));
        }

        // Wrap the bytes stream in an AlephError-typed stream
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
            /// Maps tool call stream index → call id (from first chunk with `id` field)
            index_tracker: crate::providers::delta::IndexIdTracker,
            /// Pending deltas queued from multi-delta events (e.g. `finish_reason` chunk)
            pending: VecDeque<Result<ProviderDelta>>,
            /// A terminal `Done` delta held back until the trailing
            /// `stream_options.include_usage` usage chunk arrives. `OpenAI` sends
            /// `finish_reason` and the usage chunk as *separate* chunks in that
            /// order; emitting `Done` immediately would end the stream before
            /// the usage chunk is read. Released on the usage chunk, the
            /// `[DONE]` sentinel, or HTTP stream end — always kept last so the
            /// `Done`-is-final contract holds for every consumer.
            deferred_done: Option<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
            index_tracker: crate::providers::delta::IndexIdTracker::new(),
            pending: VecDeque::new(),
            deferred_done: None,
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
                        if data == "[DONE]" {
                            // Terminal sentinel — release any deferred Done and stop.
                            if let Some(done) = state.deferred_done.take() {
                                state.pending.push_back(done);
                            }
                            state.done = true;
                        } else {
                            parse_chat_sse_event(
                                data,
                                &mut state.index_tracker,
                                &mut state.pending,
                            );
                            // Hold the terminal Done back until the trailing
                            // include_usage chunk lands, so the token count is
                            // not lost (see `defer_done_until_usage`).
                            if super::sse::defer_done_until_usage(
                                &mut state.pending,
                                &mut state.deferred_done,
                            ) {
                                state.done = true;
                            }
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
                                if data != "[DONE]" {
                                    parse_chat_sse_event(
                                        data,
                                        &mut state.index_tracker,
                                        &mut state.pending,
                                    );
                                }
                            }
                        }
                        // Reaching here means no `[DONE]` sentinel arrived (that
                        // path sets `done` and never re-polls the byte stream).
                        // A terminal signal can still exist: a deferred `Done`
                        // (finish_reason seen, trailing usage chunk lost) or a
                        // `Done` parsed from the flushed partial line. Without
                        // one, the connection dropped mid-response — surface a
                        // typed transient error instead of letting the collector
                        // default to `EndTurn` and present truncated output as a
                        // complete turn.
                        let has_terminal = state.deferred_done.is_some()
                            || crate::providers::delta::has_terminal_delta(&state.pending);
                        // Release a deferred Done that never received a
                        // trailing usage chunk or `[DONE]` sentinel.
                        if let Some(done) = state.deferred_done.take() {
                            state.pending.push_back(done);
                        }
                        if !has_terminal {
                            state.pending.push_back(Err(AlephError::Timeout {
                                suggestion: Some(
                                    "OpenAI Chat stream closed before a finish_reason or \
                                     [DONE] sentinel arrived — the response was truncated \
                                     mid-stream. Retry or switch providers."
                                        .to_string(),
                                ),
                            }));
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
                        // Append raw bytes — no UTF-8 conversion here.
                        // Conversion happens per-line when a \n is found.
                        state.line_buf.extend_from_slice(&chunk);
                        // Bound the buffer: a provider that withholds newlines
                        // must not let `line_buf` grow without limit.
                        if state.line_buf.len()
                            > crate::providers::protocols::openai_common::sse::MAX_SSE_LINE_BYTES
                        {
                            return Err(AlephError::network(format!(
                                "OpenAI Chat SSE line buffer exceeded {} bytes without a newline",
                                crate::providers::protocols::openai_common::sse::MAX_SSE_LINE_BYTES
                            )));
                        }
                        // Loop to try parsing again with the new data
                    }
                }
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

    /// Forgive common `OpenAI` model-id typos that production users hit.
    ///
    /// Endpoint-blind hook (the trait signature carries no `base_url`), so it
    /// assumes the first-party `OpenAI` host. The hot path in `build_request`
    /// instead calls [`normalize_openai_model_id`] directly with the configured
    /// `base_url`, which preserves the `openai/` slug on aggregators.
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        crate::providers::protocols::openai_common::model_id::normalize_openai_model_id(
            model_id, None,
        )
    }
}

#[cfg(test)]
mod build_request_tests {
    use super::super::OpenAiProtocol;
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::{ToolCategory, ToolDefinition};

    fn kimi_config() -> ProviderConfig {
        let mut c = ProviderConfig::test_config("Kimi-K2.7");
        c.base_url = Some("https://api.kimi.com/coding/v1".into());
        c.protocol = Some("openai".into());
        c.api_key = Some("test-key".into());
        c
    }

    fn kimi_body(model: &str, level: crate::agents::thinking::ThinkLevel) -> serde_json::Value {
        let protocol = OpenAiProtocol::new(reqwest::Client::new());
        let config = kimi_config();
        let messages = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&messages)
            .with_model(Some(model.to_string()))
            .with_think_level(Some(level));
        let req = protocol
            .build_request(&payload, &config)
            .unwrap()
            .build()
            .unwrap();
        serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap()
    }

    /// K3's headline control has to survive the whole path: `map_think_level`
    /// → `clamp_effort` → `PayloadPolicy::apply`. The endpoint gate used to
    /// delete it at the last step, so the think level vanished with no error
    /// and every request ran the vendor default. Asserting on the request body
    /// is the point — the tables agreeing proves nothing about the wire.
    #[test]
    fn kimi_k3_reasoning_effort_reaches_the_request_body() {
        use crate::agents::thinking::ThinkLevel;
        assert_eq!(
            kimi_body("k3", ThinkLevel::High)["reasoning_effort"],
            "high"
        );
        assert_eq!(
            kimi_body("k3", ThinkLevel::XHigh)["reasoning_effort"],
            "max"
        );
        assert_eq!(
            kimi_body("k3-256k", ThinkLevel::Low)["reasoning_effort"],
            "low"
        );
        // "Off" must not become `none`: on Kimi that reroutes to K2.6.
        assert_eq!(kimi_body("k3", ThinkLevel::Off)["reasoning_effort"], "low");
    }

    /// The other half of opening the endpoint gate: models that do not take
    /// the field must still never receive it.
    #[test]
    fn non_k3_kimi_models_get_no_reasoning_effort() {
        use crate::agents::thinking::ThinkLevel;
        for model in ["kimi-k2.6", "kimi-for-coding", "moonshot-v1-128k"] {
            assert!(
                kimi_body(model, ThinkLevel::High)
                    .get("reasoning_effort")
                    .is_none(),
                "{model} must not receive reasoning_effort"
            );
        }
    }

    #[test]
    fn kimi_tool_schema_derefs_refs_and_has_object_type() {
        let protocol = OpenAiProtocol::new(reqwest::Client::new());
        let schema = serde_json::json!({
            "$defs": {
                "Action": {
                    "oneOf": [
                        { "type": "string", "const": "start" },
                        { "type": "string", "const": "stop" }
                    ]
                }
            },
            "type": "object",
            "properties": {
                "action": { "$ref": "#/$defs/Action" }
            }
        });
        let tool = ToolDefinition::new("loop", "loop tool", schema, ToolCategory::Builtin);
        let messages = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&messages).with_tools(Some(std::slice::from_ref(&tool)));
        let req = protocol
            .build_request(&payload, &kimi_config())
            .unwrap()
            .build()
            .unwrap();
        let body_bytes = req.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();

        let params = &body["tools"][0]["function"]["parameters"];
        assert_eq!(params["type"], "object", "top-level type must be object");
        assert!(params.get("$defs").is_none(), "$defs should be inlined");
        assert!(
            params["properties"]["action"].get("$ref").is_none(),
            "$ref should be dereferenced"
        );
    }
}

#[cfg(test)]
mod normalize_model_id_tests {
    use super::super::OpenAiProtocol;
    use crate::providers::adapter::ProtocolAdapter;

    fn p() -> OpenAiProtocol {
        OpenAiProtocol::new(reqwest::Client::new())
    }

    #[test]
    fn rewrites_no_dash_variants() {
        let a = p();
        assert_eq!(a.normalize_model_id("gpt4o"), "gpt-4o");
        assert_eq!(a.normalize_model_id("o3mini"), "o3-mini");
        assert_eq!(a.normalize_model_id("o4mini"), "o4-mini");
    }

    #[test]
    fn strips_openai_vendor_prefix() {
        let a = p();
        assert_eq!(a.normalize_model_id("openai/gpt-4o"), "gpt-4o");
        assert_eq!(a.normalize_model_id("openai/o3-mini"), "o3-mini");
    }

    #[test]
    fn canonical_ids_pass_through_borrowed() {
        let a = p();
        // Borrowed pass-through (no allocation) for already-canonical inputs.
        let got = a.normalize_model_id("gpt-4o");
        assert!(matches!(got, std::borrow::Cow::Borrowed(_)));
        assert_eq!(got, "gpt-4o");
    }

    #[test]
    fn unknown_models_unchanged() {
        let a = p();
        assert_eq!(a.normalize_model_id("deepseek-chat"), "deepseek-chat");
        assert_eq!(a.normalize_model_id("llama-3.3-70b"), "llama-3.3-70b");
    }
}
