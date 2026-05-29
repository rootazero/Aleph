//! ProtocolAdapter trait implementation for OpenAiProtocol.

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
use crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema;
use crate::providers::protocols::openai_common::provider_policy::build_payload_policy;
use crate::providers::protocols::openai_common::response_format::to_chat_response_format;

#[async_trait]
impl ProtocolAdapter for OpenAiProtocol {
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
        let model_name = self.normalize_model_id(raw_model).into_owned();

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

        // Add reasoning_effort for thinking models
        if let Some(ref level) = payload.think_level {
            if let Some(effort) = Self::map_think_level(level) {
                body["reasoning_effort"] = json!(effort);
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
                    let mut params = td.parameters.clone();
                    if policy.capabilities.supports_strict_schema {
                        normalize_strict_schema(&mut params, true);
                    }
                    OpenAiTool {
                        tool_type: "function".into(),
                        function: OpenAiFunction {
                            name: sanitize_tool_name(&td.name),
                            description: td.description.clone(),
                            parameters: params,
                            strict: if policy.capabilities.supports_strict_schema {
                                Some(true)
                            } else {
                                None
                            },
                        },
                    }
                })
                .collect();
            body["tools"] = serde_json::to_value(&tools)
                .map_err(|e| AlephError::provider(format!("Failed to serialize tools: {}", e)))?;
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
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body))
    }

    /// Stream fine-grained delta events from the OpenAI Chat Completions SSE format.
    ///
    /// Parses SSE events from the Chat Completions streaming format and emits
    /// fine-grained [`ProviderDelta`] events. Uses the unfold+pending-queue pattern
    /// so that finish_reason chunks (which produce multiple deltas) can emit all
    /// of them without loss.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
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
                    message: format!("OpenAI Chat API rate limited (429): {}", error_text),
                    suggestion: Some(suggestion),
                });
            }
            return Err(AlephError::provider(format!(
                "OpenAI Chat API error ({}): {}",
                status, error_text
            )));
        }

        // Wrap the bytes stream in an AlephError-typed stream
        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
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
            /// Pending deltas queued from multi-delta events (e.g. finish_reason chunk)
            pending: VecDeque<Result<ProviderDelta>>,
            /// A terminal `Done` delta held back until the trailing
            /// `stream_options.include_usage` usage chunk arrives. OpenAI sends
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
                        // Release a deferred Done that never received a
                        // trailing usage chunk or `[DONE]` sentinel.
                        if let Some(done) = state.deferred_done.take() {
                            state.pending.push_back(done);
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

    /// Forgive common OpenAI model-id typos that production users hit.
    ///
    /// Covers the no-dash variants returned by some routing aggregators
    /// (`gpt4o` → `gpt-4o`, `gpt4omini` → `gpt-4o-mini`, `o3mini` → `o3-mini`)
    /// and the legacy `openai/` prefix that OpenRouter-style callers leak in.
    /// Anything else passes through unchanged.
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        let trimmed = model_id.trim();
        // Strip vendor-routing prefix sometimes used by aggregators.
        let core = trimmed.strip_prefix("openai/").unwrap_or(trimmed);
        let lower = core.to_ascii_lowercase();
        let canonical = match lower.as_str() {
            "gpt4o" => Some("gpt-4o"),
            "gpt4omini" | "gpt-4omini" | "gpt4o-mini" => Some("gpt-4o-mini"),
            "gpt4turbo" => Some("gpt-4-turbo"),
            "o1mini" => Some("o1-mini"),
            "o3mini" => Some("o3-mini"),
            "o4mini" => Some("o4-mini"),
            _ => None,
        };
        match canonical {
            Some(c) => std::borrow::Cow::Owned(c.to_string()),
            None if core.len() != trimmed.len() => std::borrow::Cow::Owned(core.to_string()),
            None => std::borrow::Cow::Borrowed(model_id),
        }
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
