//! ProtocolAdapter trait implementation for AnthropicProtocol.

use std::collections::{HashMap, VecDeque};
use axum::body::Bytes;

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason, TokenUsage};
use crate::providers::anthropic::{
    AnthropicTool, ContentBlock, ImageSource, Message, MessageContent, MessagesRequest,
    SystemBlock, ThinkingBlock,
};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::{Arc, RwLock};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use tracing::{debug, warn};

use super::{sanitize_anthropic_tool_name, AnthropicProtocol, ToolNameMap, ANTHROPIC_VERSION};
use super::sse::parse_anthropic_sse_event;
#[async_trait]
impl ProtocolAdapter for AnthropicProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        self.stream_idle_timeout_secs.store(
            config.stream_idle_timeout_secs.unwrap_or(60),
            std::sync::atomic::Ordering::Relaxed,
        );
        let actual_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        let endpoint = Self::build_endpoint(config);
        let messages = Self::convert_messages(payload.messages);

        // Per-request overrides provider config
        let max_tokens = payload
            .max_tokens
            .or(config.max_tokens)
            .unwrap_or(DEFAULT_MAX_TOKENS);

        // Apply temperature with config fallback
        let temperature = payload.temperature.or(config.temperature);

        // Build thinking config if enabled.
        //
        // Signed thinking blocks from prior assistant turns are replayed verbatim
        // by `convert_messages` (see ContentBlock::Thinking handling), so multi-turn
        // tool_use conversations now keep thinking enabled across turns.
        let thinking = payload
            .think_level
            .as_ref()
            .and_then(Self::map_think_level)
            .map(|budget| ThinkingBlock {
                thinking_type: "enabled".to_string(),
                budget_tokens: Some(budget),
                display: None,
            });

        // Convert tool definitions to Anthropic format. Tool names must satisfy
        // Anthropic's regex `^[a-zA-Z][a-zA-Z0-9_-]{0,127}$`; we sanitize on
        // outbound and remember the mapping so the streamed response can be
        // mapped back to the dispatcher's original tool names.
        let tools = payload.tools.map(|tool_defs| {
            tool_defs
                .iter()
                .map(|td| {
                    // Ensure input_schema has "type" field — required by strict
                    // backends like AWS Bedrock, which rejects schemas without it.
                    let mut schema = td.parameters.clone();
                    if let Some(obj) = schema.as_object_mut() {
                        obj.entry("type")
                            .or_insert_with(|| serde_json::json!("object"));
                    }
                    // Migrate schemars draft-07 schemas to draft 2020-12
                    crate::tools::schema_strictify::migrate_to_draft_2020_12(&mut schema);
                    let sanitized = sanitize_anthropic_tool_name(&td.name);
                    if sanitized != td.name {
                        let mut map = self.name_map.write().unwrap_or_else(|e| e.into_inner());
                        if map.insert(sanitized.clone(), td.name.clone()).is_none() {
                            warn!(
                                original = %td.name,
                                sanitized = %sanitized,
                                "Tool name sanitized for Anthropic compatibility"
                            );
                        }
                    }
                    AnthropicTool {
                        name: sanitized,
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
            model: actual_model.to_string(),
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
            model = %actual_model,
            "Building Anthropic request"
        );

        // Serialize to JSON value so we can add tool_choice if needed
        let mut body = serde_json::to_value(&request_body)
            .map_err(|e| AlephError::provider(format!("Failed to serialize request: {}", e)))?;

        // Add tool_choice if specified
        if let Some(ref choice) = payload.tool_choice {
            use crate::providers::adapter::ToolChoice;
            match choice {
                ToolChoice::Auto => {
                    body["tool_choice"] = serde_json::json!({"type": "auto"});
                }
                ToolChoice::Required => {
                    body["tool_choice"] = serde_json::json!({"type": "any"});
                }
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
            .header("anthropic-beta", Self::build_beta_headers(actual_model, Some(api_key)))
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
                    message: format!("Anthropic API rate limited (429): {}", error_text),
                    suggestion: Some(suggestion),
                });
            }
            return Err(AlephError::provider(format!(
                "Anthropic API error ({}): {}",
                status, error_text
            )));
        }

        let byte_stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .boxed();
        let idle_secs = self
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        let byte_stream = wrap_idle_timeout(byte_stream, idle_secs);

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: futures::stream::BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE byte buffer (bytes, not String — HTTP chunks may
            /// split multi-byte UTF-8 characters at arbitrary boundaries)
            line_buf: Vec<u8>,
            /// Maps content_block index (u32) → tool_use id
            block_ids: IndexIdTracker,
            /// Pending deltas queued from multi-delta events
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
            /// Sanitized → original tool name map (shared with the protocol).
            name_map: ToolNameMap,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
            block_ids: IndexIdTracker::new(),
            pending: VecDeque::new(),
            done: false,
            name_map: self.name_map.clone(),
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
                            parse_anthropic_sse_event(
                                data,
                                &mut state.block_ids,
                                &mut state.pending,
                                Some(&state.name_map),
                            );
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
                                    parse_anthropic_sse_event(
                                        data,
                                        &mut state.block_ids,
                                        &mut state.pending,
                                        Some(&state.name_map),
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
        "anthropic"
    }
}

/// Wrap a byte stream with a per-event idle watchdog.
///
/// If no event arrives within `idle_secs`, the stream emits an
/// `AlephError::Timeout` and terminates. `idle_secs == 0` disables the
/// watchdog (pass-through).
///
/// Maps `tokio_stream::Elapsed` to `AlephError::Timeout` so the error
/// flows through the existing transient-error path; the caller's retry /
/// surfacing logic is unchanged.
fn wrap_idle_timeout(
    stream: BoxStream<'static, Result<Bytes>>,
    idle_secs: u64,
) -> BoxStream<'static, Result<Bytes>> {
    if idle_secs == 0 {
        return stream;
    }
    use tokio_stream::StreamExt as _;
    let timed = stream.timeout(std::time::Duration::from_secs(idle_secs));
    let mapped = futures::StreamExt::map(timed, move |res| match res {
        Ok(inner) => inner,
        Err(_elapsed) => Err(AlephError::Timeout {
            suggestion: Some(format!(
                "Anthropic stream stalled: no SSE event received for {idle_secs}s. \
                 The upstream may be unresponsive; retry or increase \
                 ProviderConfig.stream_idle_timeout_secs."
            )),
        }),
    });
    Box::pin(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn wrap_idle_timeout_fires_after_threshold() {
        use futures::StreamExt as _;
        // Stream that never produces an item.
        let pending: futures::stream::BoxStream<'static, Result<Bytes>> =
            Box::pin(futures::stream::pending());
        let mut wrapped = wrap_idle_timeout(pending, 1);
        // Advance virtual clock past the 1s idle threshold.
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        let first = wrapped.next().await;
        match first {
            Some(Err(AlephError::Timeout { suggestion })) => {
                assert!(
                    suggestion
                        .as_deref()
                        .map(|s| s.contains("stalled"))
                        .unwrap_or(false),
                    "expected stall message, got {suggestion:?}",
                );
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wrap_idle_timeout_resets_on_event() {
        use futures::StreamExt as _;
        // Three chunks 50ms apart; idle threshold is 1s — none should trip.
        let stream = async_stream::stream! {
            for i in 0..3u8 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                yield Ok::<Bytes, AlephError>(Bytes::from(vec![i]));
            }
        };
        let boxed: futures::stream::BoxStream<'static, Result<Bytes>> = Box::pin(stream);
        let mut wrapped = wrap_idle_timeout(boxed, 1);
        let mut count = 0;
        while let Some(item) = wrapped.next().await {
            item.expect("no timeout expected");
            count += 1;
        }
        assert_eq!(count, 3, "all three chunks should pass through");
    }

    #[tokio::test(start_paused = true)]
    async fn wrap_idle_timeout_zero_disables() {
        use futures::StreamExt as _;
        // idle=0 should be a pass-through — no timeout firing.
        let stream = async_stream::stream! {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            yield Ok::<Bytes, AlephError>(Bytes::from_static(b"late"));
        };
        let boxed: futures::stream::BoxStream<'static, Result<Bytes>> = Box::pin(stream);
        let mut wrapped = wrap_idle_timeout(boxed, 0);
        tokio::time::advance(std::time::Duration::from_secs(120)).await;
        let item = wrapped.next().await;
        match item {
            Some(Ok(b)) => assert_eq!(&b[..], b"late"),
            other => panic!("expected late Ok event, got {other:?}"),
        }
    }
}


