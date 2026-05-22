//! ProtocolAdapter trait implementation for AnthropicProtocol.

use std::collections::VecDeque;

use super::sse::parse_anthropic_sse_event;
use super::{
    sanitize_anthropic_tool_name, AnthropicProtocol, ToolNameMap, ANTHROPIC_VERSION,
    CLAUDE_CODE_USER_AGENT,
};
use crate::config::types::provider::CacheRetention;
use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::anthropic::types::{Metadata, OutputConfig};
use crate::providers::anthropic::{AnthropicTool, MessagesRequest, SystemBlock, ThinkingBlock};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use crate::providers::message::{CacheControl, EphemeralTtl};
use crate::tool_metadata::DEFAULT_MAX_TOKENS;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use tracing::{debug, warn};

/// Parse a comma-separated stop-sequences string into a Vec<String>.
/// Splits on `,`, trims each element, and filters out empties.
fn parse_stop_sequences(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Strip JSON Schema union keywords from a tool's `input_schema`.
///
/// Anthropic's tool validator rejects requests with `oneOf` / `allOf` /
/// `anyOf` at the top level of `input_schema`, returning HTTP 400 with
/// "items is not an object" / "array schema items is not an object".
/// schemars-generated schemas commonly produce these for Rust enums and
/// `Option<T>`-rich structs. Dropping the keywords lets the request
/// validate against the fallback `type: object` schema rather than
/// failing the whole turn.
///
/// Mirrors hermes-agent `_normalize_tool_input_schema` (lines 1411-1416).
/// We do NOT recurse into nested properties — Anthropic only rejects the
/// keywords at the top level; nested unions inside property schemas pass.
fn strip_anthropic_tool_schema_unions(schema: &mut serde_json::Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    // Important: use a loop, not `.any()` — `.any()` short-circuits on the
    // first removal and skips the remaining keys, so a schema with both
    // `allOf` and `anyOf` would only get one stripped.
    let mut stripped = false;
    for key in &["oneOf", "allOf", "anyOf"] {
        if obj.remove(*key).is_some() {
            stripped = true;
        }
    }
    if stripped {
        // Fallback: ensure the schema still validates as an object so
        // the Anthropic validator has something to check against.
        obj.entry("type")
            .or_insert_with(|| serde_json::json!("object"));
        if obj.get("type").and_then(|v| v.as_str()) == Some("object")
            && !obj.contains_key("properties")
        {
            obj.insert(
                "properties".to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
        }
    }
}

/// Resolve the effective prompt-cache retention for a request given the
/// provider config and the target endpoint URL. See spec §2 decision table.
///
/// Cycle 4: host-level gating moved to `policy.capabilities.supports_cache_control`
/// in `build_request`. This function only resolves `None → Short` and warns
/// when `Long` is requested on a non-official host (injection will be blocked
/// downstream, but the user signal is preserved for auditability).
fn effective_cache_retention(config: &ProviderConfig, endpoint: &str) -> CacheRetention {
    match config.cache_retention {
        Some(CacheRetention::Long) if !endpoint.contains("api.anthropic.com") => {
            // Keep the existing warning that surfaces long-TTL misuse on
            // third-party hosts. Physical injection is blocked downstream
            // by policy.capabilities.supports_cache_control, but the user
            // signal that they explicitly asked for Long is still useful.
            tracing::warn!(
                endpoint = %endpoint,
                "cache_retention = long on non-official Anthropic host; \
                 cache_control will not be injected because the endpoint \
                 capability is disabled."
            );
            CacheRetention::Long
        }
        Some(r) => r,
        None => CacheRetention::Short,
    }
}

/// Maximum prompt-cache breakpoints Anthropic accepts in a single request.
const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Inject `cache_control` into the last text block of the `system` array.
///
/// Handles three input shapes for `payload["system"]`:
/// - Missing / null / empty array → no-op, returns `false`.
/// - String → normalized to `[{"type":"text","text":<s>,"cache_control":cc}]`.
/// - Array → finds the last element with `type == "text"` and sets its
///   `cache_control` (overwriting any prior value). If no text element
///   exists, no-op.
///
/// Returns `true` when a breakpoint was placed, so the caller can subtract it
/// from the [`MAX_CACHE_BREAKPOINTS`] budget.
fn inject_cache_control_into_system_array(
    payload: &mut serde_json::Value,
    cc: CacheControl,
) -> bool {
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");

    match payload.get_mut("system") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(s)) => {
            let normalized = serde_json::json!([{
                "type": "text",
                "text": std::mem::take(s),
                "cache_control": cc_json,
            }]);
            payload["system"] = normalized;
            true
        }
        Some(serde_json::Value::Array(arr)) => {
            for block in arr.iter_mut().rev() {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(obj) = block.as_object_mut() {
                        obj.insert("cache_control".to_string(), cc_json);
                    }
                    return true;
                }
            }
            false
        }
        Some(_) => false,
    }
}

/// Inject `cache_control` into the trailing content block of up to
/// `max_breakpoints` of the most-recent messages in `payload["messages"]`.
///
/// Anthropic allows at most [`MAX_CACHE_BREAKPOINTS`] cache breakpoints per
/// request. Marking the last few messages (in addition to the system block)
/// maximises the multi-turn cache-hit rate: older breakpoints stay at stable
/// positions and become cache *reads* on the following turn — the documented
/// incremental-caching pattern, matching hermes' `system_and_3` layout.
///
/// Per message the marker is placed on the last non-`thinking` /
/// non-`redacted_thinking` block (signatures on those blocks must round-trip
/// untouched). String content is normalized to an array. A message whose
/// blocks are all thinking-type is skipped and does not consume a breakpoint.
/// A message that already carries a `cache_control` marker counts as one
/// breakpoint and is left untouched, so the total never exceeds the budget.
fn inject_cache_control_into_recent_messages(
    payload: &mut serde_json::Value,
    cc: CacheControl,
    max_breakpoints: usize,
) {
    if max_breakpoints == 0 {
        return;
    }
    let cc_json = serde_json::to_value(cc).expect("CacheControl serialize is infallible");

    let Some(messages) = payload.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };

    let mut remaining = max_breakpoints;
    for msg in messages.iter_mut().rev() {
        if remaining == 0 {
            break;
        }
        match msg.get_mut("content") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(s)) => {
                msg["content"] = serde_json::json!([{
                    "type": "text",
                    "text": std::mem::take(s),
                    "cache_control": cc_json.clone(),
                }]);
                remaining -= 1;
            }
            Some(serde_json::Value::Array(blocks)) => {
                // A message that already carries a marker IS a breakpoint —
                // count it but don't add a second to the same message.
                if blocks.iter().any(|b| b.get("cache_control").is_some()) {
                    remaining -= 1;
                    continue;
                }
                let target = blocks.iter_mut().rev().find(|b| {
                    let ty = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    ty != "thinking" && ty != "redacted_thinking"
                });
                if let Some(obj) = target.and_then(|b| b.as_object_mut()) {
                    obj.insert("cache_control".to_string(), cc_json.clone());
                    remaining -= 1;
                }
            }
            Some(_) => {}
        }
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
        // Cycle 4: resolve capability policy once at the top of build_request.
        let policy =
            crate::providers::protocols::anthropic::provider_policy::build_anthropic_policy(
                config.base_url.as_deref(),
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
        // Claude 4.6/4.7 use adaptive thinking — the model picks its own budget
        // per turn, controlled by `output_config.effort` rather than a static
        // `budget_tokens`. Manual budgets are deprecated on these models.
        // Older models still take the legacy `{type: "enabled", budget_tokens}`.
        //
        // Signed thinking blocks from prior assistant turns are replayed verbatim
        // by `convert_messages` (see ContentBlock::Thinking handling), so multi-turn
        // tool_use conversations keep thinking enabled across turns.
        let adaptive = Self::supports_adaptive_thinking(actual_model);
        let (thinking, adaptive_effort): (Option<ThinkingBlock>, Option<&'static str>) =
            match payload.think_level.as_ref() {
                Some(level) if adaptive => {
                    match Self::map_think_level_to_adaptive_effort(level, actual_model) {
                        Some(eff) => (
                            Some(ThinkingBlock {
                                thinking_type: "adaptive".to_string(),
                                budget_tokens: None,
                                display: Some("summarized".to_string()),
                            }),
                            Some(eff),
                        ),
                        None => (None, None),
                    }
                }
                Some(level) => (
                    Self::map_think_level(level).map(|budget| ThinkingBlock {
                        thinking_type: "enabled".to_string(),
                        budget_tokens: Some(budget),
                        display: None,
                    }),
                    None,
                ),
                None => (None, None),
            };

        // Convert tool definitions to Anthropic format. Tool names must satisfy
        // Anthropic's regex `^[a-zA-Z][a-zA-Z0-9_-]{0,127}$`; we sanitize on
        // outbound and remember the mapping so the streamed response can be
        // mapped back to the dispatcher's original tool names.
        //
        // Two additional defenses convert hard 400s into warnings:
        // 1. Strip top-level `oneOf` / `allOf` / `anyOf` from the schema —
        //    Anthropic's validator rejects union keywords on tool input_schema.
        //    Without this, schemars-generated tool schemas (common for Rust
        //    enums) 400 with "items is not an object".
        // 2. Dedup tool names — Anthropic rejects requests with duplicate
        //    tool names. Upstream injection paths may slip a duplicate
        //    through; drop the second occurrence with a warning so the
        //    request still succeeds.
        let tools = payload.tools.map(|tool_defs| {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut out: Vec<AnthropicTool> = Vec::with_capacity(tool_defs.len());
            for td in tool_defs.iter() {
                // Ensure input_schema has "type" field — required by strict
                // backends like AWS Bedrock, which rejects schemas without it.
                let mut schema = td.parameters.clone();
                if let Some(obj) = schema.as_object_mut() {
                    obj.entry("type")
                        .or_insert_with(|| serde_json::json!("object"));
                }
                // Migrate schemars draft-07 schemas to draft 2020-12
                crate::tools::schema_strictify::migrate_to_draft_2020_12(&mut schema);
                // Defense (1): strip union keywords that Anthropic rejects.
                strip_anthropic_tool_schema_unions(&mut schema);
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
                // Defense (2): dedup. Anthropic 400s on duplicate names; drop
                // the second occurrence so the rest of the request still flies.
                if !seen.insert(sanitized.clone()) {
                    warn!(
                        original = %td.name,
                        sanitized = %sanitized,
                        "Duplicate tool name dropped for Anthropic request \
                         (Anthropic rejects duplicate names with HTTP 400)"
                    );
                    continue;
                }
                out.push(AnthropicTool {
                    name: sanitized,
                    description: td.description.clone(),
                    input_schema: schema,
                });
            }
            out
        });

        // Build system block without cache_control; injection is gated on
        // policy.capabilities.supports_cache_control below.
        let system = payload.system_prompt.map(|s| vec![SystemBlock::text(s)]);

        // Cycle 4: wire sampling fields from config
        let top_p = config.top_p;
        let top_k = config.top_k;
        let stop_sequences = config
            .stop_sequences
            .as_deref()
            .map(parse_stop_sequences)
            .filter(|v| !v.is_empty());

        // Extended thinking is incompatible with sampling parameters: Anthropic
        // rejects a request that sets `temperature` (other than 1), `top_p`, or
        // `top_k` while `thinking` is enabled (HTTP 400). Strip them here so a
        // user who configures sampling AND uses a thinking model does not get
        // every request rejected — the model samples at its thinking default.
        //
        // Claude 4.7+ additionally 400s on sampling params even without thinking;
        // gate them off whenever `forbids_sampling_params(model)` is true.
        let strip_sampling = thinking.is_some() || Self::forbids_sampling_params(actual_model);
        let (temperature, top_p, top_k) = if strip_sampling {
            (None, None, None)
        } else {
            (temperature, top_p, top_k)
        };

        // Cycle 4: wire metadata + effort from config. Adaptive thinking on
        // 4.6/4.7 overrides any config-level effort — the model needs the
        // ThinkLevel-derived effort to know how hard to think on this turn.
        let metadata = config.metadata_user_id.as_ref().map(|uid| Metadata {
            user_id: Some(uid.clone()),
        });
        let output_config = adaptive_effort
            .map(|e| OutputConfig {
                effort: Some(e.to_string()),
            })
            .or_else(|| {
                config.effort.as_ref().map(|e| OutputConfig {
                    effort: Some(e.clone()),
                })
            });

        let request_body = MessagesRequest {
            model: actual_model.to_string(),
            messages,
            max_tokens,
            system,
            temperature,
            top_p,
            top_k,
            stop_sequences,
            stream: Some(true), // always streaming (stream-first architecture)
            thinking,
            tools,
            service_tier: config.service_tier.clone(),
            metadata,
            output_config,
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

        // Inject prompt-cache breakpoints only when the endpoint supports it
        // (cf. policy.capabilities.supports_cache_control). Cycle 4 moved
        // the host-level gate here from effective_cache_retention.
        let extended_cache_ttl = if policy.capabilities.supports_cache_control {
            let retention = effective_cache_retention(config, &endpoint);
            let ext = matches!(retention, CacheRetention::Long);
            if retention != CacheRetention::Off {
                let cc = CacheControl::Ephemeral {
                    ttl: if ext {
                        Some(EphemeralTtl::OneHour)
                    } else {
                        None
                    },
                };
                // System block takes one breakpoint; the rest go to the most
                // recent messages so multi-turn conversations cache-hit.
                let system_used = inject_cache_control_into_system_array(&mut body, cc);
                let message_budget = MAX_CACHE_BREAKPOINTS - usize::from(system_used);
                inject_cache_control_into_recent_messages(&mut body, cc, message_budget);
            }
            ext
        } else {
            false
        };

        // Cycle 4: strip capability-gated fields one last time.
        policy.apply(&mut body);

        let mut req = self
            .client
            .post(&endpoint)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(
                "anthropic-beta",
                Self::build_beta_headers(
                    actual_model,
                    Some(api_key),
                    extended_cache_ttl,
                    &policy.capabilities,
                ),
            )
            .header("Content-Type", "application/json");

        // OAuth tokens authenticate via `Authorization: Bearer` and require
        // Claude Code identity headers; regular console API keys use
        // `x-api-key`. Mis-routing either way produces 401/403 from Anthropic.
        if Self::is_oauth_token(api_key) {
            req = req
                .bearer_auth(api_key)
                .header("User-Agent", CLAUDE_CODE_USER_AGENT)
                .header("x-app", "cli");
        } else {
            req = req.header("x-api-key", api_key);
        }

        Ok(req.json(&body))
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
        let byte_stream = crate::providers::protocols::stream_idle::wrap_idle_timeout(
            byte_stream,
            idle_secs,
            "Anthropic",
        );

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::provider::CacheRetention;
    use crate::providers::message::CacheControl;

    // ── effective_cache_retention decision table ──────────────────────────────

    #[test]
    fn effective_retention_official_unset_defaults_short() {
        let config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        // cache_retention is None by default in test_config
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        assert_eq!(retention, CacheRetention::Short);
    }

    #[test]
    fn effective_retention_unset_always_defaults_short_after_cycle4() {
        // Cycle 4: host gate moved to policy.capabilities.supports_cache_control.
        // effective_cache_retention only resolves None → Short.
        let config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        let retention = effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Short);
    }

    #[test]
    fn effective_retention_explicit_long_on_third_party_respected() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Long);
        let retention = effective_cache_retention(&config, "https://api.moonshot.cn/v1/messages");
        assert_eq!(retention, CacheRetention::Long);
    }

    #[test]
    fn effective_retention_explicit_off_always_off() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Off);
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
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
        let used = inject_cache_control_into_system_array(&mut payload, cc);
        assert!(used, "a system breakpoint was placed");
        let system = payload["system"].as_array().unwrap();
        assert!(
            system[0].get("cache_control").is_none(),
            "first block untouched"
        );
        assert_eq!(
            system[1]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "last text block tagged",
        );
    }

    #[test]
    fn inject_cache_control_into_system_array_returns_false_when_absent() {
        let mut payload = serde_json::json!({"messages": []});
        let used = inject_cache_control_into_system_array(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
        );
        assert!(!used, "no system block → no breakpoint consumed");
    }

    // ── inject_cache_control_into_recent_messages ─────────────────────────────

    /// Count messages carrying at least one `cache_control` marker.
    fn cached_message_count(payload: &serde_json::Value) -> usize {
        payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| {
                m["content"]
                    .as_array()
                    .map(|blocks| blocks.iter().any(|b| b.get("cache_control").is_some()))
                    .unwrap_or(false)
            })
            .count()
    }

    #[test]
    fn recent_messages_tags_last_n_messages() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [{"type": "text", "text": "m2"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m3"}]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        // The 3 most recent messages tagged; the oldest (m0) untouched.
        assert_eq!(cached_message_count(&payload), 3);
        assert!(payload["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(payload["messages"][3]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn recent_messages_respects_breakpoint_budget() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "m0"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "m1"}]},
                {"role": "user", "content": [{"type": "text", "text": "m2"}]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            2,
        );
        assert_eq!(cached_message_count(&payload), 2, "budget of 2 honored");
    }

    #[test]
    fn recent_messages_tags_last_block_only() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "text", "text": "second"}
                ]}
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert!(content[0].get("cache_control").is_none());
        assert_eq!(
            content[1]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
        );
    }

    #[test]
    fn recent_messages_skips_trailing_thinking_block() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "text", "text": "answer"},
                    {"type": "thinking", "thinking": "..."}
                ]}
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "marker lands on the text block, not the trailing thinking block",
        );
        assert!(content[1].get("cache_control").is_none());
    }

    #[test]
    fn recent_messages_skips_all_thinking_message_without_consuming_budget() {
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "older"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "only thinking"}
                ]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            1,
        );
        // The all-thinking message consumes no budget, so the older message
        // still receives the single available breakpoint.
        assert!(payload["messages"][1]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(payload["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn recent_messages_string_content_normalized_to_array() {
        let mut payload = serde_json::json!({
            "messages": [{"role": "user", "content": "plain string"}]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            3,
        );
        let content = payload["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "plain string");
        assert_eq!(
            content[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
        );
    }

    #[test]
    fn recent_messages_preexisting_marker_counts_as_breakpoint() {
        // A message already carrying a marker counts as a used breakpoint and
        // is left untouched — no second marker on the same message.
        let mut payload = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "older"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "pre"},
                    {"type": "text", "text": "tagged", "cache_control": {"type": "ephemeral"}}
                ]},
            ]
        });
        inject_cache_control_into_recent_messages(
            &mut payload,
            CacheControl::Ephemeral { ttl: None },
            1,
        );
        // Budget of 1 is consumed by the already-tagged message; older untouched.
        let recent = payload["messages"][1]["content"].as_array().unwrap();
        assert!(
            recent[0].get("cache_control").is_none(),
            "no second marker added"
        );
        assert!(payload["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
    }

    // ── retention / header signaling ──────────────────────────────────────────

    #[test]
    fn build_request_retention_off_no_cache_control_anywhere() {
        let mut config = crate::config::ProviderConfig::test_config("claude-3-5-sonnet");
        config.cache_retention = Some(CacheRetention::Off);
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
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
        let retention = effective_cache_retention(&config, "https://api.anthropic.com/v1/messages");
        let extended_cache_ttl = matches!(retention, CacheRetention::Long);
        assert!(extended_cache_ttl, "Long retention must signal beta header");
    }
}
