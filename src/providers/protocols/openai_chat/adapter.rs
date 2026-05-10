//! ProtocolAdapter trait implementation for OpenAiProtocol.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, ToolChoice};
use crate::providers::delta::ProviderDelta;
use crate::providers::openai::{
    OpenAiFunction, OpenAiTool,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt as _;
use futures::TryStreamExt;
use serde_json::json;
use std::collections::VecDeque;
use tracing::debug;

use super::{sanitize_tool_name, OpenAiProtocol};
use super::sse::parse_chat_sse_event;

#[async_trait]
impl ProtocolAdapter for OpenAiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let messages = Self::convert_messages(payload.messages, payload.system_prompt);

        // Build request body — always streaming (stream-first architecture)
        let mut body = json!({
            "model": payload.model.as_deref().unwrap_or_else(|| config.default_model()),
            "messages": messages,
            "stream": true,
        });

        // Add optional parameters (per-request overrides provider config)
        if let Some(max_tokens) = payload.max_tokens.or(config.max_tokens) {
            body["max_tokens"] = json!(max_tokens);
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

        // Add tool definitions for function calling
        if let Some(tool_defs) = payload.tools {
            let tools: Vec<OpenAiTool> = tool_defs
                .iter()
                .map(|td| {
                    // Ensure parameters has "type" field — required by strict
                    // backends like AWS Bedrock, which rejects schemas without it.
                    let mut params = td.parameters.clone();
                    if let Some(obj) = params.as_object_mut() {
                        obj.entry("type").or_insert_with(|| json!("object"));
                        // Strict OpenAI-compatible backends (T8Star, etc.) reject
                        // object schemas that omit `properties`. schemars elides
                        // the field entirely when every struct field is hidden
                        // via #[schemars(skip)] (e.g. AgentListArgs).
                        obj.entry("properties").or_insert_with(|| json!({}));
                    }
                    // Migrate schemars draft-07 schemas to draft 2020-12 for
                    // Bedrock and other strict backends.
                    crate::tools::schema_strictify::migrate_to_draft_2020_12(&mut params);
                    OpenAiTool {
                        tool_type: "function".into(),
                        function: OpenAiFunction {
                            // OpenAI API requires tool names to match ^[a-zA-Z0-9_-]+$
                            // Aleph tool names now use underscores natively; sanitize
                            // is kept as a safety net for external/plugin tool names.
                            name: sanitize_tool_name(&td.name),
                            description: td.description.clone(),
                            parameters: params,
                            strict: if td.strict { Some(true) } else { None },
                        },
                    }
                })
                .collect();
            body["tools"] = serde_json::to_value(&tools)
                .map_err(|e| AlephError::provider(format!("Failed to serialize tools: {}", e)))?;
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

        // Validate API key
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %payload.model.as_deref().unwrap_or_else(|| config.default_model()),
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
            /// Set to true after a terminal event to stop the stream
            done: bool,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
            index_tracker: crate::providers::delta::IndexIdTracker::new(),
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
                        if data != "[DONE]" {
                            parse_chat_sse_event(
                                data,
                                &mut state.index_tracker,
                                &mut state.pending,
                            );
                            // If a Done was queued, stop after draining pending
                            if state
                                .pending
                                .iter()
                                .any(|d| matches!(d, Ok(ProviderDelta::Done(_))))
                            {
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
}
