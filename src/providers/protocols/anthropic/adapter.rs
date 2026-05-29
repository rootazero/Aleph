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
use crate::thinker::prompt_builder::SystemPromptPart;
use crate::tool_metadata::DEFAULT_MAX_TOKENS;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use tracing::{debug, warn};

mod cache;
use cache::{
    effective_cache_retention, inject_cache_control_into_recent_messages,
    inject_cache_control_into_system_array, promote_system_marker_ttl, MAX_CACHE_BREAKPOINTS,
};

/// Collapse a `SystemPromptPart` slice into Anthropic `SystemBlock`s,
/// placing the cache breakpoint at the stable/dynamic boundary.
///
/// Returns `(Some(blocks), pre_placed)` where `pre_placed` is `true`
/// when a `cache_control` marker was already attached to the stable
/// tail (so the caller subtracts one from the breakpoint budget and
/// skips the legacy `inject_cache_control_into_system_array` call).
///
/// Falls back to the legacy single-block shape when `blocks` is `None`,
/// keeping every pre-existing caller wire-compatible.
fn split_system_blocks_for_cache(
    blocks: Option<&[SystemPromptPart]>,
    legacy: Option<&str>,
) -> (Option<Vec<SystemBlock>>, bool) {
    // Legacy path: identical to the pre-wiring behaviour.
    let Some(parts) = blocks else {
        let sys = legacy.map(|s| vec![SystemBlock::text(s)]);
        return (sys, false);
    };

    // Collapse consecutive parts into two strings — `stable` then `dynamic`.
    // Reasonix's ImmutablePrefix model: the boundary is data-driven by the
    // layer's `stability()`, not by string-shape heuristics.
    let mut stable = String::new();
    let mut dynamic = String::new();
    for part in parts {
        if part.cache {
            stable.push_str(&part.content);
        } else {
            dynamic.push_str(&part.content);
        }
    }

    let mut out = Vec::with_capacity(2);
    let mut pre_placed = false;
    if !stable.is_empty() {
        out.push(SystemBlock::cached_text(stable));
        pre_placed = true;
    }
    if !dynamic.is_empty() {
        out.push(SystemBlock::text(dynamic));
    }
    // All-empty corner case — also covers the situation where every layer
    // returned empty content. Treat as "no system prompt at all".
    if out.is_empty() {
        return (None, false);
    }
    (Some(out), pre_placed)
}

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
        let raw_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        let normalized_model = self.normalize_model_id(raw_model);
        let actual_model = normalized_model.as_ref();
        let endpoint = Self::build_endpoint(config);
        let messages = Self::convert_messages(payload.messages);

        // Per-request overrides provider config
        let max_tokens = payload
            .max_tokens
            .or(config.max_tokens)
            .unwrap_or(DEFAULT_MAX_TOKENS);

        // Apply temperature with config fallback, then route through the
        // preset's temperature_policy (e.g. Kimi-for-coding server-managed
        // endpoints require the field to be omitted entirely).
        let raw_temperature = payload.temperature.or(config.temperature);
        let temperature = crate::providers::presets::temperature_for_base_url(
            config.base_url.as_deref(),
            raw_temperature,
        );

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
        // mapped back to the tool layer's original tool names.
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

        // Build system block(s). Two shapes are supported:
        //
        // 1. Cache-first split (preferred): caller supplied `system_blocks`
        //    via `PromptBuilder::build_system_prompt_cached()`. We collapse
        //    the contiguous `cache:true` parts into a SINGLE stable block
        //    carrying the cache breakpoint, and the remaining `cache:false`
        //    parts into a SINGLE dynamic tail block with no marker. Per
        //    Anthropic semantics everything UP TO AND INCLUDING the marker
        //    is the cacheable prefix; the dynamic tail therefore does NOT
        //    break the prefix hash when its content changes turn-to-turn
        //    (e.g. RuntimeContext.current_time, tool_runtime_state).
        //
        // 2. Legacy single-string: caller used `system_prompt`. Wrapped as
        //    one block; `inject_cache_control_into_system_array` then puts
        //    the marker on it later (the whole assembly becomes the cache
        //    prefix — strictly worse, kept for unmigrated callers).
        let (system, pre_placed_system_breakpoint) =
            split_system_blocks_for_cache(payload.system_blocks, payload.system_prompt);

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
                //
                // Cache-first path: `split_system_blocks_for_cache` already
                // placed the marker on the stable tail, so don't double-inject
                // — just charge one breakpoint to the budget. Legacy path:
                // marker not yet placed, so run the injector which targets
                // the last text block of the (single-element) system array.
                let system_used = if pre_placed_system_breakpoint {
                    // Stable-tail breakpoint already set on the cached_text
                    // block via `SystemBlock::cached_text()`. If the active
                    // retention is `Long`, overwrite the (short, no-TTL)
                    // marker with the 1h ephemeral variant so the user's
                    // cache_retention setting is honoured on the split path.
                    if ext {
                        promote_system_marker_ttl(&mut body, cc);
                    }
                    true
                } else {
                    inject_cache_control_into_system_array(&mut body, cc)
                };
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

    /// Forgive dotted variants of Claude model ids.
    ///
    /// Users commonly write `claude-3.5-sonnet` (matching the marketing name)
    /// but the API expects `claude-3-5-sonnet`. Only the segment after
    /// `claude-` is rewritten so dates/families with legit dots are untouched.
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        let trimmed = model_id.trim();
        let core = trimmed.strip_prefix("anthropic/").unwrap_or(trimmed);
        if let Some(suffix) = core.strip_prefix("claude-") {
            // Replace dot-separated version segments (e.g. `3.5`) with dashes,
            // but only when both sides are digits — protects ISO dates.
            let mut out = String::with_capacity(core.len() + 8);
            out.push_str("claude-");
            let mut chars = suffix.chars().peekable();
            let mut prev_was_digit = false;
            while let Some(c) = chars.next() {
                if c == '.' && prev_was_digit && chars.peek().is_some_and(|n| n.is_ascii_digit()) {
                    out.push('-');
                    prev_was_digit = false;
                    continue;
                }
                prev_was_digit = c.is_ascii_digit();
                out.push(c);
            }
            if out != model_id {
                return std::borrow::Cow::Owned(out);
            }
        }
        if core.len() != trimmed.len() {
            return std::borrow::Cow::Owned(core.to_string());
        }
        std::borrow::Cow::Borrowed(model_id)
    }
}

#[cfg(test)]
mod normalize_model_id_tests {
    use super::super::AnthropicProtocol;
    use crate::providers::adapter::ProtocolAdapter;

    fn p() -> AnthropicProtocol {
        AnthropicProtocol::new(reqwest::Client::new())
    }

    #[test]
    fn rewrites_dotted_version_in_claude_family() {
        let a = p();
        assert_eq!(
            a.normalize_model_id("claude-3.5-sonnet"),
            "claude-3-5-sonnet"
        );
        assert_eq!(
            a.normalize_model_id("claude-3.7-sonnet-latest"),
            "claude-3-7-sonnet-latest"
        );
    }

    #[test]
    fn strips_anthropic_vendor_prefix() {
        let a = p();
        assert_eq!(
            a.normalize_model_id("anthropic/claude-3-5-sonnet"),
            "claude-3-5-sonnet"
        );
    }

    #[test]
    fn iso_dated_models_unchanged() {
        let a = p();
        // ISO-style dated suffixes contain digits-then-letter, no dots — never matches.
        assert_eq!(
            a.normalize_model_id("claude-sonnet-4-5-20250514"),
            "claude-sonnet-4-5-20250514"
        );
        assert_eq!(
            a.normalize_model_id("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn non_claude_models_pass_through_borrowed() {
        let a = p();
        let got = a.normalize_model_id("kimi-k2-0905-preview");
        assert!(matches!(got, std::borrow::Cow::Borrowed(_)));
    }
}
