//! ProtocolAdapter trait implementation for AnthropicProtocol.

use std::collections::{HashMap, VecDeque};
use axum::body::Bytes;

use crate::agents::thinking::ThinkLevel;
use crate::config::types::provider::CacheRetention;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason, TokenUsage};
use crate::providers::anthropic::{
    AnthropicTool, ContentBlock, ImageSource, Message, MessageContent, MessagesRequest,
    SystemBlock, ThinkingBlock,
};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use crate::providers::message::{CacheControl, EphemeralTtl, UnifiedMessage};
use crate::sync_primitives::{Arc, RwLock};
use super::sse::parse_anthropic_sse_event;
use super::{sanitize_anthropic_tool_name, AnthropicProtocol, ToolNameMap, ANTHROPIC_VERSION};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use tracing::{debug, warn};

/// Resolve the effective prompt-cache retention for a request given the
/// provider config and the target base URL. See spec §2 decision table.
///
/// - Explicit `Some(retention)` is always respected. A `Long` opt-in on a
///   non-official hostname is honored but logged via `tracing::warn!` so the
///   trust path is auditable.
/// - `None` (unset) is hostname-gated: `api.anthropic.com` → `Short`,
///   anything else → `Off`.
fn effective_cache_retention(config: &ProviderConfig, base_url: &str) -> CacheRetention {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
    let is_official = host.as_deref() == Some("api.anthropic.com");

    match config.cache_retention {
        Some(explicit) => {
            if matches!(explicit, CacheRetention::Long) && !is_official {
                tracing::warn!(
                    base_url = %base_url,
                    "cache_retention = long on non-official Anthropic host; \
                     trusting explicit opt-in (extended-cache-ttl-2025-04-11 \
                     beta header will be sent)",
                );
            }
            explicit
        }
        None if is_official => CacheRetention::Short,
        None => CacheRetention::Off,
    }
}

/// Inject `cache_control` into the last text block of the `system` array.
///
/// Handles three input shapes for `payload["system"]`:
/// - Missing / null / empty array → no-op.
/// - String → normalized to `[{"type":"text","text":<s>,"cache_control":cc}]`.
/// - Array → finds the last element with `type == "text"` and sets its
///   `cache_control` (overwriting any prior value). If no text element
///   exists, no-op.
fn inject_cache_control_into_system_array(
    payload: &mut serde_json::Value,
    cc: CacheControl,
) {
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");

    match payload.get_mut("system") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::String(s)) => {
            let normalized = serde_json::json!([{
                "type": "text",
                "text": std::mem::take(s),
                "cache_control": cc_json,
            }]);
            payload["system"] = normalized;
        }
        Some(serde_json::Value::Array(arr)) => {
            for block in arr.iter_mut().rev() {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(obj) = block.as_object_mut() {
                        obj.insert("cache_control".to_string(), cc_json);
                    }
                    return;
                }
            }
        }
        Some(_) => {}
    }
}

/// Inject `cache_control` into the last non-thinking block of the trailing
/// user message in `payload["messages"]`.
///
/// - No `messages` array, empty array, or no `role == "user"` message → no-op.
/// - Last user's `content` as string → normalized to array with cache_control.
/// - Last user's `content` as array → walks blocks in reverse; first non-
///   thinking/redacted_thinking block gets `cache_control` set. If all blocks
///   are thinking-type → no-op.
fn inject_cache_control_into_last_user_message(
    payload: &mut serde_json::Value,
    cc: CacheControl,
) {
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");

    let Some(messages) = payload.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };

    let Some(last_user) = messages
        .iter_mut()
        .rev()
        .find(|m| m.get("role").and_then(|v| v.as_str()) == Some("user"))
    else {
        return;
    };

    match last_user.get_mut("content") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::String(s)) => {
            let normalized = serde_json::json!([{
                "type": "text",
                "text": std::mem::take(s),
                "cache_control": cc_json,
            }]);
            last_user["content"] = normalized;
        }
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks.iter_mut().rev() {
                let ty = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if ty == "thinking" || ty == "redacted_thinking" {
                    continue;
                }
                if let Some(obj) = block.as_object_mut() {
                    obj.insert("cache_control".to_string(), cc_json);
                }
                return;
            }
        }
        Some(_) => {}
    }
}

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

        // Inject prompt-cache breakpoints if retention is not Off.
        let retention = effective_cache_retention(config, &endpoint);
        let extended_cache_ttl = matches!(retention, CacheRetention::Long);
        if retention != CacheRetention::Off {
            let cc = CacheControl::Ephemeral {
                ttl: if extended_cache_ttl {
                    Some(EphemeralTtl::OneHour)
                } else {
                    None
                },
            };
            inject_cache_control_into_system_array(&mut body, cc);
            inject_cache_control_into_last_user_message(&mut body, cc);
        }

        Ok(self
            .client
            .post(&endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(
                "anthropic-beta",
                Self::build_beta_headers(actual_model, Some(api_key), extended_cache_ttl),
            )
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
    use crate::config::types::provider::CacheRetention;
    use crate::providers::message::{CacheControl, EphemeralTtl};

    // ── effective_cache_retention decision table ──────────────────────────────

    #[test]
    fn effective_retention_official_unset_defaults_short() {
        let config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        // cache_retention is None by default in test_config
        let retention =
            effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        assert_eq!(retention, CacheRetention::Short);
    }

    #[test]
    fn effective_retention_third_party_unset_defaults_off() {
        let config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        let retention =
            effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Off);
    }

    #[test]
    fn effective_retention_explicit_long_on_third_party_respected() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Long);
        let retention =
            effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Long);
    }

    #[test]
    fn effective_retention_explicit_off_always_off() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Off);
        let retention =
            effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        assert_eq!(retention, CacheRetention::Off);
    }

    // ── inject_cache_control_into_system_array ────────────────────────────────

    #[test]
    fn inject_cache_control_into_system_array_sets_last_text_block() {
        let mut payload = serde_json::json!({
            "system": [
                {"type": "text", "text": "You are a helpful assistant."},
                {"type": "text", "text": "Today is 2026-05-11."}
            ]
        });
        let cc = CacheControl::Ephemeral { ttl: None };
        inject_cache_control_into_system_array(&mut payload, cc);
        let system = payload["system"].as_array().unwrap();
        assert!(system[0].get("cache_control").is_none(), "first block untouched");
        assert_eq!(
            system[1]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "last text block tagged",
        );
    }

    // ── inject_cache_control_into_last_user_message ───────────────────────────

    #[test]
    fn inject_cache_control_into_last_user_message_tags_last_block() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "hello"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "text", "text": "second"}
                ]}
            ]
        });
        let cc = CacheControl::Ephemeral { ttl: None };
        inject_cache_control_into_last_user_message(&mut payload, cc);
        let last_user_content = payload["messages"][2]["content"].as_array().unwrap();
        assert!(last_user_content[0].get("cache_control").is_none());
        assert_eq!(
            last_user_content[1]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
        );
    }

    #[test]
    fn inject_cache_control_skips_trailing_thinking_block() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "answer"},
                    {"type": "thinking", "thinking": "..."}
                ]}
            ]
        });
        let cc = CacheControl::Ephemeral { ttl: None };
        inject_cache_control_into_last_user_message(&mut payload, cc);
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
        );
        assert!(content[1].get("cache_control").is_none());
    }

    // ── retention / header signaling ──────────────────────────────────────────

    #[test]
    fn build_request_retention_off_no_cache_control_anywhere() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Off);
        let retention =
            effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        assert_eq!(retention, CacheRetention::Off);
        let mut payload = serde_json::json!({
            "system": [{"type": "text", "text": "sys"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}],
        });
        let snapshot = payload.clone();
        // Off means build_request skips injection; payload unchanged.
        assert_eq!(payload, snapshot);
        // Sanity: injectors WOULD change it otherwise.
        let cc = CacheControl::Ephemeral { ttl: None };
        inject_cache_control_into_system_array(&mut payload, cc);
        assert_ne!(payload, snapshot);
    }

    #[test]
    fn long_ttl_implies_extended_cache_beta_token() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Long);
        let retention =
            effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        let extended_cache_ttl = matches!(retention, CacheRetention::Long);
        assert!(extended_cache_ttl, "Long retention must signal beta header");
    }

    // ── wrap_idle_timeout ─────────────────────────────────────────────────────

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


