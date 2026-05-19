//! ProtocolAdapter trait implementation for GeminiProtocol.

use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::delta::ProviderDelta;
use crate::providers::gemini::schema::clean_schema_for_gemini;
use crate::providers::gemini::{
    GeminiFunctionDeclaration, GeminiToolConfig,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use std::collections::VecDeque;
use tracing::debug;

use super::GeminiProtocol;
use super::sse::parse_gemini_sse_chunk;

#[async_trait]
impl ProtocolAdapter for GeminiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config, payload.model.as_deref());
        let contents = Self::convert_messages(payload.messages);
        let system_instruction = Self::build_system_instruction(payload.system_prompt);

        // Build generation config
        let thinking_config = payload.think_level.as_ref().and_then(|level| {
            Self::map_think_level(
                level,
                payload
                    .model
                    .as_deref()
                    .unwrap_or_else(|| config.default_model()),
            )
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
        };

        // Build tool declarations if provided
        let tools = payload.tools.map(|tool_defs| {
            let declarations: Vec<GeminiFunctionDeclaration> = tool_defs
                .iter()
                .map(|td| {
                    let mut params = td.parameters.clone();
                    // Sanitize schema for Gemini's restricted OpenAPI subset
                    clean_schema_for_gemini(&mut params);
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
    /// IDs (`gemini_fc_{counter}`) are generated as fallback.
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
                    message: format!("Gemini API rate limited (429): {}", error_text),
                    suggestion: Some(suggestion),
                });
            }
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
            /// Incomplete SSE byte buffer (bytes, not String — HTTP chunks may
            /// split multi-byte UTF-8 characters at arbitrary boundaries)
            line_buf: Vec<u8>,
            /// Monotonically increasing counter for synthetic tool call IDs
            fc_counter: u64,
            /// Pending deltas queued from multi-delta events
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
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
                        // Append raw bytes — no UTF-8 conversion here.
                        // Conversion happens per-line when a \n is found.
                        state.line_buf.extend_from_slice(&chunk);
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "gemini"
    }
}
