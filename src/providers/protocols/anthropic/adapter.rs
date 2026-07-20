//! `ProtocolAdapter` trait implementation for `AnthropicProtocol`.

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
use crate::providers::protocols::anthropic::provider_policy::{
    is_kimi_anthropic_base_url, normalize_kimi_coding_model_id, strip_cache_control,
    KIMI_CODING_USER_AGENT,
};
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
/// Splits on `,`, trims each element, and filters out empties. Capped at 4 —
/// the Anthropic API rejects requests carrying more than 4 stop sequences.
fn parse_stop_sequences(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(4)
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

/// True if `pending` holds a terminal delta — a `Done` (the model finished or
/// paused) or an `Error` (the provider reported a fault). Either means the
/// stream reached a defined end, so the truncation guard must NOT fire.
fn queue_has_terminal(pending: &VecDeque<Result<ProviderDelta>>) -> bool {
    pending
        .iter()
        .any(|d| matches!(d, Ok(ProviderDelta::Done(_)) | Ok(ProviderDelta::Error(_))))
}

/// Decide whether a now-closed Anthropic stream was truncated mid-flight.
///
/// `saw_terminal` — a `Done`/`Error` was observed earlier in the stream.
/// `tail_terminal` — the flushed final (newline-less) line produced one.
///
/// Anthropic always emits a terminal `message_delta` (→ `Done`) before closing
/// a healthy stream, so a close with neither flag set means the body was cut
/// mid-flight. Pure predicate, lifted out of the `stream_deltas` unfold so the
/// guard is unit-testable.
const fn stream_was_truncated(saw_terminal: bool, tail_terminal: bool) -> bool {
    !saw_terminal && !tail_terminal
}

/// Parse Anthropic's error envelope `{"type":"error","error":{"type","message"}}`.
/// Returns `(error_type, message)`; either is `None` when the body is not that
/// shape (e.g. an HTML 502 from an upstream proxy, or an empty body).
fn parse_anthropic_error_envelope(body: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return (None, None);
    };
    let err = v.get("error");
    let etype = err
        .and_then(|e| e.get("type"))
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let emsg = err
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string);
    (etype, emsg)
}

/// Map an Anthropic non-2xx HTTP response to a typed [`AlephError`] carrying the
/// correct retry semantics (cf. [`crate::providers::retry`]).
///
/// Mirrors hermes-agent / pi typed error handling, mapped onto Rust's error
/// enum so classification is total and compiler-checked rather than ad-hoc
/// string sniffing:
/// - `401` / `403` (or `authentication_error` / `permission_error`) → auth, fatal
/// - `429` (or `rate_limit_error`) → distinct rate-limit, fatal (retry amplifies)
/// - `529` (or `overloaded_error`) → transient, **retryable**
/// - `400` / `422` (or `invalid_request_error`) → client error, fatal
/// - `5xx` / `api_error` → keeps the numeric status so the retry policy's
///   status-code list (`500/502/503/504`) decides
///
/// The previous inline logic only special-cased `429` and otherwise emitted a
/// bare `provider("... ({status}): {body}")`, which left `529` (not in the
/// default retryable status list) classified as fatal and surfaced raw JSON
/// bodies to the user.
fn classify_anthropic_error_response(
    status: u16,
    retry_after: Option<&str>,
    body: &str,
) -> AlephError {
    let (etype, emsg) = parse_anthropic_error_envelope(body);
    let detail = emsg.unwrap_or_else(|| {
        if body.trim().is_empty() {
            "(no response body)".to_string()
        } else {
            body.to_string()
        }
    });
    let overloaded = status == 529 || etype.as_deref() == Some("overloaded_error");
    match status {
        401 | 403 => AlephError::authentication(
            "anthropic",
            format!("Anthropic authentication failed ({status}): {detail}"),
        ),
        429 => {
            let suggestion = retry_after.map_or_else(
                || "Rate limited. Wait before retrying or upgrade your API plan.".to_string(),
                |ra| format!("Rate limited. Retry after {ra} seconds."),
            );
            AlephError::RateLimitError {
                message: format!("Anthropic API rate limited (429): {detail}"),
                suggestion: Some(suggestion),
            }
        }
        // Overloaded is transient. Keep the literal "overloaded" token in the
        // message so `retry::is_overloaded_message` classifies it retryable —
        // 529 is deliberately absent from the default retryable status list.
        _ if overloaded => {
            AlephError::provider(format!("Anthropic API overloaded ({status}): {detail}"))
        }
        // Client errors: the request is malformed; retrying just re-fails.
        400 | 422 => {
            AlephError::provider(format!("Anthropic invalid request ({status}): {detail}"))
        }
        // Everything else (incl. 500/502/503/504) keeps the numeric status in
        // the message so the retry policy's status-code match decides.
        _ => AlephError::provider(format!("Anthropic API error ({status}): {detail}")),
    }
}

#[async_trait]
impl ProtocolAdapter for AnthropicProtocol {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder> {
        self.stream_idle_timeout_secs.store(
            config.stream_idle_timeout_secs.unwrap_or(crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS),
            std::sync::atomic::Ordering::Relaxed,
        );
        // Cycle 4: resolve capability policy once at the top of build_request.
        let policy =
            crate::providers::protocols::anthropic::provider_policy::build_anthropic_policy(
                config.base_url.as_deref(),
            );
        // OAuth tokens take a different auth header AND require the Claude Code
        // identity system block (see `prepend_claude_code_identity`). Resolve
        // the bit once here so both the system-block and auth-header paths agree.
        let is_oauth = config.api_key.as_deref().is_some_and(Self::is_oauth_token);
        let raw_model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| config.default_model());
        let normalized_model = self.normalize_model_id(raw_model);
        let mut actual_model = normalized_model.as_ref();
        // The Kimi Coding endpoint only serves the single model id
        // `kimi-for-coding`; display-style ids like `Kimi-K2.7` and legacy
        // aliases (`kimi-code`, `k2p5`) must be mapped to it.
        let is_kimi = is_kimi_anthropic_base_url(config.base_url.as_deref());
        if is_kimi {
            actual_model = normalize_kimi_coding_model_id(actual_model);
        }
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
                // Explicit `Off` on an adaptive (4.6+) model: these models
                // think by DEFAULT, so omitting the `thinking` field leaves it
                // enabled — the caller's "off" would be silently ignored. Emit
                // `{type:"disabled"}` to actually turn it off (pi parity:
                // `thinkingEnabled === false → thinking:{type:"disabled"}`).
                // Generation-5 models (fable-5) 400 on an explicit disabled
                // block — omission is the only off-switch there, so they get
                // no `thinking` field at all. Older/non-adaptive models
                // default to no-thinking, so the generic arms below disable
                // them by omission as before.
                Some(crate::agents::thinking::ThinkLevel::Off) if adaptive => {
                    if Self::omits_disabled_thinking(actual_model) {
                        (None, None)
                    } else {
                        (
                            Some(ThinkingBlock {
                                thinking_type: "disabled".to_string(),
                                budget_tokens: None,
                                display: None,
                            }),
                            None,
                        )
                    }
                }
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
                // Legacy `{type:"enabled", budget_tokens}` thinking (Claude
                // 3.7–4.5). Gated on the model actually having a thinking mode:
                // Claude 3.5 / 3.0 have none and 400 on this block, so a cheap
                // non-reasoning model carrying a configured think level must
                // omit it rather than fail the whole request.
                Some(level) if Self::supports_extended_thinking(actual_model) => (
                    Self::map_think_level(level).map(|budget| ThinkingBlock {
                        thinking_type: "enabled".to_string(),
                        budget_tokens: Some(budget),
                        display: None,
                    }),
                    None,
                ),
                // Known non-thinking model: ignore the think level entirely.
                Some(_) => (None, None),
                None => (None, None),
            };

        // Legacy thinking carves `budget_tokens` out of `max_tokens`; Anthropic
        // 400s when the budget meets/exceeds the cap. Raise the cap to keep a
        // visible answer's worth of output above the budget. No-op for adaptive
        // thinking (budget_tokens: None) and for non-thinking turns.
        let max_tokens = Self::adjust_max_tokens_for_thinking_budget(max_tokens, thinking.as_ref());

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
                // rust-doctor-disable-next-line excessive-clone
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
                    // rust-doctor-disable-next-line excessive-clone
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
                // rust-doctor-disable-next-line excessive-clone
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
                    // rust-doctor-disable-next-line excessive-clone
                    description: td.description.clone(),
                    input_schema: schema,
                });
            }
            out
        });

        // Build system block(s). Two shapes are supported:
        //
        // 1. Cache-first split (preferred): caller supplied `system_blocks`
        //    via `PromptBuilder::build_system_prompt_cached_with_mode()`. We collapse
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

        // OAuth requests must lead with the Claude Code identity block, or
        // Anthropic rejects them (401/403). Prepend it ahead of any caller
        // blocks — it joins the cacheable prefix without moving the existing
        // breakpoint, which stays on the stable block (see helper docs).
        let system = if is_oauth {
            Some(Self::prepend_claude_code_identity(system))
        } else {
            system
        };

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
        //
        // Only *enabled*/*adaptive* thinking conflicts with sampling — an
        // explicit `{type:"disabled"}` block leaves sampling perfectly legal, so
        // don't strip temperature just because a (disabled) thinking block is
        // present.
        let thinking_active = thinking
            .as_ref()
            .is_some_and(|t| t.thinking_type != "disabled");
        let strip_sampling = thinking_active || Self::forbids_sampling_params(actual_model);
        let (temperature, top_p, top_k) = if strip_sampling {
            (None, None, None)
        } else {
            (temperature, top_p, top_k)
        };

        // Cycle 4: wire metadata + effort from config. Adaptive thinking on
        // 4.6/4.7 overrides any config-level effort — the model needs the
        // ThinkLevel-derived effort to know how hard to think on this turn.
        let metadata = config.metadata_user_id.as_ref().map(|uid| Metadata {
            // rust-doctor-disable-next-line excessive-clone
            user_id: Some(uid.clone()),
        });
        let output_config = adaptive_effort
            .map(|e| OutputConfig {
                effort: Some(e.to_string()),
            })
            .or_else(|| {
                config.effort.as_ref().map(|e| OutputConfig {
                    // rust-doctor-disable-next-line excessive-clone
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
            // rust-doctor-disable-next-line excessive-clone
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
            .map_err(|e| AlephError::provider(format!("Failed to serialize request: {e}")))?;

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
            // The 1h TTL is an Anthropic-1P beta: third-party Anthropic-protocol
            // hosts (Bedrock/Azure) accept plain ephemeral markers but reject
            // the `ttl` key under strict schema validation — gate the TTL (and
            // via `ext`, its beta header) on the official endpoint, mirroring
            // the OpenAI adapter's EndpointClass::OpenAiPublic gate on
            // prompt_cache_retention. `Long` elsewhere degrades to the default
            // 5m marker: still cached, never a 400.
            let ext =
                matches!(retention, CacheRetention::Long) && endpoint.contains("api.anthropic.com");
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

        // Kimi Coding rejects pre-placed cache_control markers on system blocks
        // and message content, even though it does not advertise cache support.
        // Mirrors openclaw's `stripAnthropicCacheControlMarkers`.
        if is_kimi {
            if let Some(system) = body.get_mut("system") {
                strip_cache_control(system);
            }
            if let Some(messages) = body.get_mut("messages") {
                strip_cache_control(messages);
            }
        }

        let beta_header = Self::build_beta_headers(
            actual_model,
            Some(api_key),
            extended_cache_ttl,
            &policy.capabilities,
        );
        let mut req = self
            .client
            .post(&endpoint)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json");
        // Custom-compatible endpoints (e.g. Kimi Coding) may reject an empty or
        // unknown `anthropic-beta` header with a 429 overload response. Only
        // send the header when there is at least one real beta feature.
        if !beta_header.is_empty() {
            req = req.header("anthropic-beta", beta_header);
        }

        // OAuth tokens authenticate via `Authorization: Bearer` and require
        // Claude Code identity headers; regular console API keys use
        // `x-api-key`. Mis-routing either way produces 401/403 from Anthropic.
        // `is_oauth` was resolved up-front so the system-block and auth paths
        // never disagree.
        if is_oauth {
            req = req
                .bearer_auth(api_key)
                .header("User-Agent", CLAUDE_CODE_USER_AGENT)
                .header("x-app", "cli");
        } else {
            req = req.header("x-api-key", api_key);
            // Kimi Coding expects the claude-code User-Agent even for API-key
            // auth; without it the endpoint may return 429 "engine overloaded".
            if is_kimi {
                req = req.header("User-Agent", KIMI_CODING_USER_AGENT);
            }
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
    /// `message_delta` with `stop_reason` + usage) can emit all deltas without loss.
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
            let error_text =
                crate::providers::protocols::http_client::read_error_body(response).await;
            return Err(classify_anthropic_error_response(
                status.as_u16(),
                retry_after.as_deref(),
                &error_text,
            ));
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
            "Anthropic",
        );

        /// Per-iteration mutable state carried through unfold
        struct State {
            bytes: futures::stream::BoxStream<'static, Result<axum::body::Bytes>>,
            /// Incomplete SSE byte buffer (bytes, not String — HTTP chunks may
            /// split multi-byte UTF-8 characters at arbitrary boundaries)
            line_buf: Vec<u8>,
            /// Maps `content_block` index (u32) → `tool_use` id
            block_ids: IndexIdTracker,
            /// Pending deltas queued from multi-delta events
            pending: VecDeque<Result<ProviderDelta>>,
            /// Set to true after a terminal event to stop the stream
            done: bool,
            /// Set to true once any terminal event (`Done` from `message_delta`
            /// or an `error` SSE frame) has been seen. Distinct from `done`,
            /// which fires only on `Done`. Used to detect a truncated stream:
            /// if the HTTP body ends while this is still `false`, the response
            /// was cut mid-flight and must surface as a retryable error rather
            /// than collapsing to the collector's default `EndTurn` stop reason
            /// (a silent partial success). Mirrors pi's
            /// `sawMessageStart && !sawMessageEnd` guard.
            saw_terminal: bool,
            /// Sanitized → original tool name map (shared with the protocol).
            name_map: ToolNameMap,
        }

        let state = State {
            bytes: byte_stream,
            line_buf: Vec::new(),
            block_ids: IndexIdTracker::new(),
            pending: VecDeque::new(),
            done: false,
            saw_terminal: false,
            // rust-doctor-disable-next-line excessive-clone
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
                            // Track terminals: a `Done`/`Error` here means the
                            // stream completed (or reported a fault) cleanly, so
                            // the HTTP-end branch must not flag truncation.
                            if queue_has_terminal(&state.pending) {
                                state.saw_terminal = true;
                            }
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
                        // Truncation guard: Anthropic always emits a terminal
                        // `message_delta` (carrying `stop_reason`) before the
                        // stream closes. If the HTTP body ended without one — and
                        // the flushed tail produced none either — the response was
                        // cut mid-flight (connection drop, proxy/idle timeout). Left
                        // unflagged the `DeltaCollector` defaults to `EndTurn`, so a
                        // partial answer is silently accepted as a clean finish and
                        // never retried. Surface a retryable network error instead
                        // (classified transient by `providers::retry`), mirroring
                        // pi's `sawMessageStart && !sawMessageEnd` throw.
                        if stream_was_truncated(
                            state.saw_terminal,
                            queue_has_terminal(&state.pending),
                        ) {
                            state.pending.push_back(Err(AlephError::network(
                                "Anthropic stream ended before completion (no \
                                 message_stop / stop_reason) — response was truncated \
                                 mid-flight; retrying",
                            )));
                        }
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

/// Tests for the stream-truncation guard (pi `sawMessageStart && !sawMessageEnd`
/// parity). Drives raw SSE event sequences through the real parser and the
/// guard predicates to confirm a healthy stream is left untouched while a
/// stream cut before its terminal `message_delta` is flagged for retry.
#[cfg(test)]
mod truncation_guard_tests {
    use super::super::sse::parse_anthropic_sse_event;
    use super::super::ToolNameMap;
    use super::{queue_has_terminal, stream_was_truncated};
    use crate::error::Result;
    use crate::providers::delta::{IndexIdTracker, ProviderDelta};
    use crate::sync_primitives::{Arc, RwLock};
    use std::collections::{HashMap, VecDeque};

    /// Replay a sequence of SSE `data` payloads exactly as the `stream_deltas`
    /// unfold would (accumulating `saw_terminal` and a final-tail check), and
    /// return whether the truncation guard would fire at stream close.
    fn would_flag_truncation(events: &[&str]) -> bool {
        let mut block_ids = IndexIdTracker::new();
        let name_map: ToolNameMap = Arc::new(RwLock::new(HashMap::new()));
        let mut saw_terminal = false;
        let mut last: VecDeque<Result<ProviderDelta>> = VecDeque::new();
        for data in events {
            let mut pending = VecDeque::new();
            parse_anthropic_sse_event(data, &mut block_ids, &mut pending, Some(&name_map));
            if queue_has_terminal(&pending) {
                saw_terminal = true;
            }
            last = pending;
        }
        // `last` stands in for the flushed final line's output at HTTP close.
        stream_was_truncated(saw_terminal, queue_has_terminal(&last))
    }

    #[test]
    fn predicate_truth_table() {
        assert!(stream_was_truncated(false, false));
        assert!(!stream_was_truncated(true, false));
        assert!(!stream_was_truncated(false, true));
        assert!(!stream_was_truncated(true, true));
    }

    #[test]
    fn queue_has_terminal_recognizes_done_and_error_only() {
        let mut q: VecDeque<Result<ProviderDelta>> = VecDeque::new();
        q.push_back(Ok(ProviderDelta::TextDelta("hi".into())));
        assert!(!queue_has_terminal(&q), "text is not terminal");
        q.push_back(Ok(ProviderDelta::Done(
            crate::providers::adapter::StopReason::EndTurn,
        )));
        assert!(queue_has_terminal(&q), "Done is terminal");

        let mut e: VecDeque<Result<ProviderDelta>> = VecDeque::new();
        e.push_back(Ok(ProviderDelta::Error("overloaded".into())));
        assert!(queue_has_terminal(&e), "Error is terminal");
    }

    #[test]
    fn complete_text_stream_is_not_flagged() {
        // Healthy stream: ends with a terminal message_delta carrying stop_reason.
        let events = [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        assert!(!would_flag_truncation(&events));
    }

    #[test]
    fn text_stream_cut_before_message_delta_is_flagged() {
        // Connection dropped after some text but before the terminal
        // message_delta — the real-world truncation this guard catches.
        let events = [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Partial answer that never finis"}}"#,
        ];
        assert!(would_flag_truncation(&events));
    }

    #[test]
    fn empty_stream_is_flagged() {
        // A 200 response that closes with zero events is abnormal — flag it.
        assert!(would_flag_truncation(&[]));
    }

    #[test]
    fn error_frame_suppresses_truncation_flag() {
        // The provider reported an explicit error then closed — that is the
        // terminal signal; the guard must defer to it, not double-report.
        let events = [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ];
        assert!(!would_flag_truncation(&events));
    }
}

/// Tests for typed HTTP error-response classification (hermes-agent / pi typed
/// error parity). Confirms each Anthropic error maps to the right `AlephError`
/// variant AND the right retry verdict via `providers::retry`.
#[cfg(test)]
mod error_classification_tests {
    use super::{classify_anthropic_error_response, parse_anthropic_error_envelope};
    use crate::error::AlephError;
    use crate::providers::retry::retryable_reason;

    fn is_retryable(e: &AlephError) -> bool {
        retryable_reason(e).is_some()
    }

    #[test]
    fn parses_error_envelope() {
        let (t, m) = parse_anthropic_error_envelope(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert_eq!(t.as_deref(), Some("overloaded_error"));
        assert_eq!(m.as_deref(), Some("Overloaded"));
    }

    #[test]
    fn non_json_body_yields_no_envelope_fields() {
        let (t, m) = parse_anthropic_error_envelope("<html>502 Bad Gateway</html>");
        assert!(t.is_none() && m.is_none());
    }

    #[test]
    fn auth_errors_are_typed_and_fatal() {
        for status in [401u16, 403] {
            let e = classify_anthropic_error_response(status, None, "");
            assert!(matches!(e, AlephError::AuthenticationError { .. }));
            assert!(!is_retryable(&e), "auth must not retry");
        }
    }

    #[test]
    fn rate_limit_is_typed_with_retry_after_and_not_retryable() {
        let e = classify_anthropic_error_response(
            429,
            Some("12"),
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        );
        match &e {
            AlephError::RateLimitError { suggestion, .. } => {
                assert!(suggestion.as_ref().unwrap().contains("12"));
            }
            other => panic!("expected RateLimitError, got {other:?}"),
        }
        assert!(
            !is_retryable(&e),
            "rate limit retry amplifies — must be fatal"
        );
    }

    #[test]
    fn overloaded_529_is_retryable_even_with_empty_body() {
        // The core gap: 529 is NOT in the default retryable status list, so the
        // old bare provider("... (529): ") was classified fatal. The classifier
        // injects the "overloaded" token so retry treats it transient.
        let e = classify_anthropic_error_response(529, None, "");
        assert!(matches!(e, AlephError::ProviderError { .. }));
        assert!(is_retryable(&e), "529 overloaded must be retryable");
    }

    #[test]
    fn overloaded_by_envelope_type_on_500_is_retryable() {
        let e = classify_anthropic_error_response(
            500,
            None,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert!(is_retryable(&e));
    }

    #[test]
    fn invalid_request_400_is_fatal() {
        let e = classify_anthropic_error_response(
            400,
            None,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad tool schema"}}"#,
        );
        assert!(matches!(e, AlephError::ProviderError { .. }));
        assert!(!is_retryable(&e), "400 client error must not retry");
        // The clean envelope message is surfaced, not raw JSON.
        assert!(format!("{e}").contains("bad tool schema"));
    }

    #[test]
    fn server_5xx_remains_retryable_via_status_list() {
        for status in [500u16, 502, 503, 504] {
            let e = classify_anthropic_error_response(status, None, "upstream down");
            assert!(is_retryable(&e), "{status} should retry");
        }
    }
}
