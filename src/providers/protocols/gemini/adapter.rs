//! `ProtocolAdapter` trait implementation for `GeminiProtocol`.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::delta::ProviderDelta;
use crate::providers::gemini::schema::clean_schema_for_gemini;
use crate::providers::gemini::{GeminiFunctionDeclaration, GeminiToolConfig};
use crate::tool_metadata::DEFAULT_MAX_TOKENS;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use std::collections::VecDeque;
use tracing::debug;

use super::sse::{parse_gemini_error_body, parse_gemini_sse_chunk};
use super::GeminiProtocol;

#[async_trait]
impl ProtocolAdapter for GeminiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        self.stream_idle_timeout_secs.store(
            crate::providers::protocols::stream_idle::effective_idle_secs(config),
            std::sync::atomic::Ordering::Relaxed,
        );
        let raw_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        let normalized_model = self.normalize_model_id(raw_model);
        let endpoint = Self::build_endpoint(config, Some(normalized_model.as_ref()));
        let contents = Self::convert_messages(payload.messages);
        let system_instruction = Self::build_system_instruction(payload.system_prompt);

        // Build generation config
        let thinking_config = payload
            .think_level
            .as_ref()
            .and_then(|level| Self::map_think_level(level, normalized_model.as_ref()));

        // Stop sequences: parse the comma-separated provider-config value
        // (same convention as the OpenAI-chat adapter).
        let stop_sequences = config.stop_sequences.as_ref().and_then(|raw| {
            let seqs: Vec<String> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (!seqs.is_empty()).then_some(seqs)
        });

        // Media resolution: map the validated LOW/MEDIUM/HIGH config value to
        // Gemini's `MEDIA_RESOLUTION_*` enum. Unknown values are dropped.
        let media_resolution = config.media_resolution.as_ref().and_then(|raw| {
            match raw.trim().to_uppercase().as_str() {
                "LOW" => Some("MEDIA_RESOLUTION_LOW".to_string()),
                "MEDIUM" => Some("MEDIA_RESOLUTION_MEDIUM".to_string()),
                "HIGH" => Some("MEDIA_RESOLUTION_HIGH".to_string()),
                _ => None,
            }
        });

        // Per-request overrides provider config
        let generation_config = crate::providers::gemini::GenerationConfig {
            max_output_tokens: payload
                .max_tokens
                .or(config.max_tokens)
                .or(Some(DEFAULT_MAX_TOKENS)),
            temperature: payload.temperature.or(config.temperature),
            top_p: config.top_p,
            top_k: config.top_k,
            thinking_config,
            stop_sequences,
            media_resolution,
        };

        // Build tool declarations if provided
        let tools = payload.tools.map(|tool_defs| {
            let declarations: Vec<GeminiFunctionDeclaration> = tool_defs
                .iter()
                .map(|td| {
                    // rust-doctor-disable-next-line excessive-clone
                    let mut params = td.parameters.clone();
                    // Sanitize schema for Gemini's restricted OpenAPI subset
                    clean_schema_for_gemini(&mut params);
                    GeminiFunctionDeclaration {
                        // rust-doctor-disable-next-line excessive-clone
                        name: td.name.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        description: td.description.clone(),
                        parameters: params,
                    }
                })
                .collect();
            vec![GeminiToolConfig {
                function_declarations: declarations,
            }]
        });

        let request_body = crate::providers::gemini::GenerateContentRequest {
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
            model = %payload.model.as_deref().unwrap_or_else(|| config.default_model()),
            "Building Gemini request"
        );

        // Always request the SSE stream (stream-first architecture). The API
        // key travels in the `x-goog-api-key` header (added below), not the
        // URL, so it never leaks into logs, proxies, or tracing spans.
        let mut url = endpoint;
        url.push_str("?alt=sse");

        // Serialize to JSON value so we can add tool_config if needed
        let mut body = serde_json::to_value(&request_body)
            .map_err(|e| AlephError::provider(format!("Failed to serialize request: {e}")))?;

        // Add tool_config if tool_choice is specified
        if let Some(ref choice) = payload.tool_choice {
            use crate::providers::adapter::ToolChoice;
            body["tool_config"] = match choice {
                ToolChoice::Auto => {
                    serde_json::json!({"function_calling_config": {"mode": "AUTO"}})
                }
                ToolChoice::Required => {
                    serde_json::json!({"function_calling_config": {"mode": "ANY"}})
                }
                ToolChoice::Specific(name) => serde_json::json!({"function_calling_config": {
                    "mode": "ANY", "allowed_function_names": [name]
                }}),
                ToolChoice::None => {
                    serde_json::json!({"function_calling_config": {"mode": "NONE"}})
                }
            };
        }

        Ok(self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", api_key)
            .json(&body))
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    /// Stream fine-grained [`ProviderDelta`] events from a Gemini SSE response.
    ///
    /// Gemini streams complete function calls per chunk (not incremental args), so
    /// each `functionCall` part yields `ToolCallStart + ToolCallArgDelta + ToolCallEnd`
    /// in one shot. Native call IDs are used when present (Gemini 3+); synthetic
    /// IDs (`gemini_fc_{counter}_{nonce}`) are generated as fallback.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>> {
        let status = response.status();
        if !status.is_success() {
            let header_retry_after =
                crate::providers::protocols::http_client::retry_after_secs(response.headers());
            let error_text =
                crate::providers::protocols::http_client::read_error_body(response).await;
            // Parse Gemini's error envelope for a clean message; fall back to raw text.
            let parsed = parse_gemini_error_body(&error_text);
            let detail = parsed.as_ref().map_or_else(
                // rust-doctor-disable-next-line excessive-clone
                || error_text.clone(),
                |e| format!("{} ({})", e.message, e.status),
            );
            // Prefer the `Retry-After` header; otherwise fall back to the
            // `google.rpc.RetryInfo` carried in the error details — Gemini
            // commonly returns the authoritative backoff there and omits the
            // header entirely.
            let retry_after =
                header_retry_after.or_else(|| parsed.as_ref().and_then(|e| e.retry_delay_secs()));
            if status.as_u16() == 429 {
                let suggestion = retry_after.as_ref().map_or_else(
                    || "Rate limited. Wait before retrying or upgrade your API plan.".to_string(),
                    |ra| format!("Rate limited. Retry after {ra} seconds."),
                );
                return Err(AlephError::RateLimitError {
                    message: format!("Gemini API rate limited (429): {detail}"),
                    suggestion: Some(suggestion),
                });
            }
            return Err(AlephError::provider(format!(
                "Gemini API error ({status}): {detail}"
            )));
        }

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
            "Gemini",
        );

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE byte buffer (bytes, not String — HTTP chunks may
            /// split multi-byte UTF-8 characters at arbitrary boundaries)
            line_buf: Vec<u8>,
            /// Monotonically increasing counter for synthetic tool call IDs
            fc_counter: u64,
            /// Pending deltas queued from multi-delta events
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
            /// Whether a terminal (`Done`/`Error`) was ever queued. A healthy
            /// Gemini stream ends with `[DONE]`; an EOF without one means the
            /// body was cut mid-flight (proxy kill, provider fault) and must
            /// surface as a typed Timeout, not a normal EndTurn.
            saw_terminal: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
            fc_counter: 0,
            pending: VecDeque::new(),
            done: false,
            saw_terminal: false,
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
                        if data != "[DONE]" {
                            parse_gemini_sse_chunk(data, &mut state.fc_counter, &mut state.pending);
                            // If Done was queued, stop after draining pending
                            if state
                                .pending
                                .iter()
                                .any(|d| matches!(d, Ok(ProviderDelta::Done(_))))
                            {
                                state.done = true;
                            }
                            if state.pending.iter().any(|d| {
                                matches!(
                                    d,
                                    Ok(ProviderDelta::Done(_)) | Ok(ProviderDelta::Error(_))
                                )
                            }) {
                                state.saw_terminal = true;
                            }
                        }
                    }
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
                                    parse_gemini_sse_chunk(
                                        data,
                                        &mut state.fc_counter,
                                        &mut state.pending,
                                    );
                                }
                                if data != "[DONE]"
                                    && state.pending.iter().any(|d| {
                                        matches!(
                                            d,
                                            Ok(ProviderDelta::Done(_))
                                                | Ok(ProviderDelta::Error(_))
                                        )
                                    })
                                {
                                    state.saw_terminal = true;
                                }
                            }
                        }
                        state.done = true;
                        if let Some(delta) = state.pending.pop_front() {
                            return Some((delta, state));
                        }
                        // Truncation guard: no terminal signal before EOF means
                        // the body was cut mid-flight. Surface a typed Timeout
                        // (the failover/retry path classifies it as transient)
                        // instead of silently ending the turn as if the model
                        // finished.
                        if !state.saw_terminal {
                            return Some((
                                Err(AlephError::Timeout {
                                    suggestion: Some(
                                        "Gemini stream ended before a terminal [DONE] was seen — \
                                         the connection was cut mid-response. Retry or switch \
                                         providers."
                                            .to_string(),
                                    ),
                                }),
                                state,
                            ));
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
                                "Gemini SSE line buffer exceeded {} bytes without a newline",
                                crate::providers::protocols::openai_common::sse::MAX_SSE_LINE_BYTES
                            )));
                        }
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "gemini"
    }

    /// Forgive common Gemini model-id variants.
    ///
    /// * Drops the `models/` API-path prefix that callers sometimes leak.
    /// * Drops the `google/` vendor prefix used by aggregators.
    /// * Rewrites the legacy `gemini-pro-1.5` ordering to `gemini-1.5-pro`
    ///   (same for `gemini-flash-1.5`, etc.) — Google flipped the family /
    ///   version order in late 2024 and SDKs sometimes lag.
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        let trimmed = model_id.trim();
        let stripped = trimmed
            .strip_prefix("models/")
            .or_else(|| trimmed.strip_prefix("google/"))
            .unwrap_or(trimmed);

        // Pattern: gemini-<family>-<version>  ⇒  gemini-<version>-<family>
        // where family ∈ {pro, flash, ultra} and version is "1.5"/"2.5" etc.
        if let Some(rest) = stripped.strip_prefix("gemini-") {
            let mut parts = rest.splitn(3, '-');
            if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                let is_family = matches!(a, "pro" | "flash" | "ultra");
                let is_version =
                    b.contains('.') && b.chars().next().is_some_and(|c| c.is_ascii_digit());
                if is_family && is_version {
                    let tail = parts.next();
                    let canonical = match tail {
                        Some(t) => format!("gemini-{b}-{a}-{t}"),
                        None => format!("gemini-{b}-{a}"),
                    };
                    return std::borrow::Cow::Owned(canonical);
                }
            }
        }

        if stripped.len() != trimmed.len() {
            return std::borrow::Cow::Owned(stripped.to_string());
        }
        std::borrow::Cow::Borrowed(model_id)
    }
}

#[cfg(test)]
mod normalize_model_id_tests {
    use super::super::GeminiProtocol;
    use crate::providers::adapter::ProtocolAdapter;

    fn p() -> GeminiProtocol {
        GeminiProtocol::new(reqwest::Client::new())
    }

    #[test]
    fn strips_models_and_google_prefix() {
        let a = p();
        assert_eq!(
            a.normalize_model_id("models/gemini-2.5-flash"),
            "gemini-2.5-flash"
        );
        assert_eq!(
            a.normalize_model_id("google/gemini-2.5-pro"),
            "gemini-2.5-pro"
        );
    }

    #[test]
    fn flips_legacy_family_first_ordering() {
        let a = p();
        assert_eq!(a.normalize_model_id("gemini-pro-1.5"), "gemini-1.5-pro");
        assert_eq!(a.normalize_model_id("gemini-flash-1.5"), "gemini-1.5-flash");
    }

    #[test]
    fn canonical_form_passes_through() {
        let a = p();
        let got = a.normalize_model_id("gemini-2.5-flash");
        assert!(matches!(got, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn dated_or_unknown_unchanged() {
        let a = p();
        assert_eq!(
            a.normalize_model_id("gemini-2.5-flash-lite"),
            "gemini-2.5-flash-lite"
        );
        assert_eq!(a.normalize_model_id("text-bison-001"), "text-bison-001");
    }
}
