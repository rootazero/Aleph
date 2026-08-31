//! `AnthropicProtocol` implementation — construction and internal helpers.

use std::collections::HashMap;

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::anthropic::{
    ContentBlock, ImageSource, Message, MessageContent, SystemBlock, ThinkingBlock,
};
use crate::providers::message::UnifiedMessage;
use crate::providers::protocols::anthropic::provider_policy::AnthropicCapabilities;
use crate::sync_primitives::{Arc, RwLock};
use reqwest::Client;

use super::{sanitize_anthropic_tool_name, AnthropicProtocol, CLAUDE_CODE_IDENTITY};

/// Minimum output budget reserved when extended thinking is enabled.
///
/// Anthropic rejects (HTTP 400) any request where `max_tokens <=
/// thinking.budget_tokens` — the budget is carved *out of* `max_tokens`, so at
/// least some room must remain for the visible answer. Mirrors openclaw
/// `adjustMaxTokensForThinking` (`minOutputTokens = 1024`).
const MIN_OUTPUT_TOKENS_WITH_THINKING: u32 = 1024;
impl AnthropicProtocol {
    /// Create a new Anthropic protocol adapter
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self {
            client,
            name_map: Arc::new(RwLock::new(HashMap::new())),
            stream_idle_timeout_secs: Arc::new(crate::sync_primitives::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }

    /// Build the endpoint URL
    pub(super) fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map_or_else(
                || "https://api.anthropic.com".to_string(),
                |s| s.to_string(),
            );

        // Defence in depth: reject non-HTTP schemes before reqwest sees the URL.
        if let Err(e) = crate::providers::protocols::http_client::validate_provider_base_url(
            &raw_base_url,
        ) {
            tracing::error!(error = %e, "Anthropic provider base_url failed validation");
        }

        // Normalize URL
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        format!("{base_url}/v1/messages")
    }

    /// Convert `UnifiedMessages` to Anthropic Messages
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub(super) fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Message> {
        let mut result = Vec::new();
        let mut i = 0;
        while i < messages.len() {
            match &messages[i] {
                UnifiedMessage::User { content } => {
                    // rust-doctor-disable-next-line unnecessary-allocation
                    let mut blocks = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text {
                                text,
                                cache_control,
                            } => {
                                blocks.push(ContentBlock::Text {
                                    // rust-doctor-disable-next-line excessive-clone
                                    text: text.clone(),
                                    // Pass the marker through verbatim — the unified
                                    // and wire types are the same struct now, so a
                                    // pre-placed 1h TTL is no longer silently
                                    // downgraded to the 5m default.
                                    cache_control: *cache_control,
                                });
                            }
                            crate::providers::message::ContentBlock::Image { data, mime_type } => {
                                blocks.push(ContentBlock::Image {
                                    source: ImageSource {
                                        source_type: "base64".to_string(),
                                        // rust-doctor-disable-next-line excessive-clone
                                        media_type: mime_type.clone(),
                                        // rust-doctor-disable-next-line excessive-clone
                                        data: data.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    let image_count = blocks
                        .iter()
                        .filter(|b| matches!(b, ContentBlock::Image { .. }))
                        .count();
                    if image_count > 0 {
                        tracing::info!(
                            target: "multimodal",
                            probe = "P6_provider",
                            role = "user",
                            content_type = "multimodal",
                            image_count = image_count,
                            "Anthropic multimodal message converted"
                        );
                    }
                    // Anthropic API rejects messages with empty content (HTTP 400:
                    // "must not be empty"). Emit a single-space placeholder so historical
                    // empty-turn artifacts (e.g. tokens=0 streaming aborts) don't poison
                    // subsequent requests.
                    if blocks.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: " ".to_string(),
                            cache_control: None,
                        });
                    }
                    if blocks.len() == 1 {
                        if let ContentBlock::Text { text, .. } = &blocks[0] {
                            result.push(Message {
                                role: "user".to_string(),
                                content: MessageContent::Text {
                                    // rust-doctor-disable-next-line excessive-clone
                                    content: text.clone(),
                                },
                            });
                            i += 1;
                            continue;
                        }
                    }
                    result.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal { content: blocks },
                    });
                    i += 1;
                }
                UnifiedMessage::Assistant { content } => {
                    // rust-doctor-disable-next-line unnecessary-allocation
                    let mut blocks = Vec::new();
                    // Track the most recent signed thinking block so we can inject
                    // reasoning_content into the next ToolUse when thinking is enabled.
                    let mut pending_thinking: Option<String> = None;
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text {
                                text,
                                cache_control,
                            } => {
                                if !text.trim().is_empty() {
                                    blocks.push(ContentBlock::Text {
                                        // rust-doctor-disable-next-line excessive-clone
                                        text: text.clone(),
                                        // Verbatim passthrough — TTL preserved (see the
                                        // user-branch comment above).
                                        cache_control: *cache_control,
                                    });
                                }
                            }
                            crate::providers::message::ContentBlock::Thinking {
                                thinking,
                                signature: Some(sig),
                            } => {
                                // Replay the signed thinking block when we have its signature.
                                // Anthropic requires a verbatim replay (thinking + signature)
                                // whenever the assistant turn also carries tool_use blocks.
                                // Without a signature the API would reject the message, so
                                // drop unsigned thinking — providers that don't sign (Gemini,
                                // OpenAI) never produce it for an Anthropic-bound turn.
                                if !thinking.is_empty() {
                                    blocks.push(ContentBlock::Thinking {
                                        // rust-doctor-disable-next-line excessive-clone
                                        thinking: thinking.clone(),
                                        // rust-doctor-disable-next-line excessive-clone
                                        signature: sig.clone(),
                                    });
                                    // Remember this thinking for the next ToolCall
                                    // rust-doctor-disable-next-line excessive-clone
                                    pending_thinking = Some(thinking.clone());
                                }
                            }
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                                ..
                            } => {
                                // Sanitize tool_use_id for Anthropic
                                let sanitized_id: String = id
                                    .chars()
                                    .map(|c| {
                                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                            c
                                        } else {
                                            '_'
                                        }
                                    })
                                    .take(64)
                                    .collect();
                                // Anthropic API requires input to be a dictionary, never a string.
                                // When thinking is enabled and precedes a tool call, we must
                                // include reasoning_content in the tool_use input or the API
                                // rejects the request with:
                                //   "thinking is enabled but reasoning_content is missing
                                //    in assistant tool call message".
                                let mut input = if arguments.is_object() {
                                    // rust-doctor-disable-next-line excessive-clone
                                    arguments.clone()
                                } else {
                                    serde_json::json!({})
                                };
                                if let Some(ref reasoning) = pending_thinking {
                                    if let Some(obj) = input.as_object_mut() {
                                        obj.insert(
                                            "reasoning_content".to_string(),
                                            // rust-doctor-disable-next-line excessive-clone
                                            serde_json::Value::String(reasoning.clone()),
                                        );
                                    }
                                    // Keep pending_thinking set: Anthropic requires
                                    // reasoning_content in EVERY tool_use block that
                                    // follows a signed thinking block within the same
                                    // assistant message, not just the first one.
                                }
                                blocks.push(ContentBlock::ToolUse {
                                    id: sanitized_id,
                                    name: sanitize_anthropic_tool_name(name),
                                    input,
                                });
                            }
                            _ => {}
                        }
                    }
                    // Anthropic API rejects messages with empty content (HTTP 400:
                    // "must not be empty"). Emit a single-space placeholder so historical
                    // empty-turn artifacts (e.g. tokens=0 streaming aborts) don't poison
                    // subsequent requests.
                    if blocks.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: " ".to_string(),
                            cache_control: None,
                        });
                    }
                    result.push(Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Multimodal { content: blocks },
                    });
                    i += 1;
                }
                UnifiedMessage::ToolResult { .. } => {
                    // Collect consecutive ToolResults into one user message.
                    // Any image blocks they carry (e.g. a `desktop` screenshot)
                    // are emitted as sibling image blocks AFTER the tool_result
                    // blocks in the same user turn — Anthropic accepts trailing
                    // images in a tool-result turn, and this is what finally lets
                    // a vision model see the screen it acted on.
                    // rust-doctor-disable-next-line unnecessary-allocation
                    let mut tool_blocks = Vec::new();
                    // rust-doctor-disable-next-line unnecessary-allocation
                    let mut image_blocks = Vec::new();
                    while i < messages.len() {
                        if let UnifiedMessage::ToolResult {
                            tool_call_id,
                            content,
                            is_error,
                            ..
                        } = &messages[i]
                        {
                            // rust-doctor-disable-next-line unnecessary-allocation
                            let mut parts = Vec::new();
                            for b in content {
                                match b {
                                    crate::providers::message::ContentBlock::Text {
                                        text, ..
                                    // rust-doctor-disable-next-line excessive-clone
                                    } => parts.push(text.clone()),
                                    crate::providers::message::ContentBlock::Json { value } => {
                                        parts
                                            .push(serde_json::to_string(value).unwrap_or_default());
                                    }
                                    crate::providers::message::ContentBlock::Image {
                                        data,
                                        mime_type,
                                    } => image_blocks.push(ContentBlock::Image {
                                        source: ImageSource {
                                            source_type: "base64".to_string(),
                                            // rust-doctor-disable-next-line excessive-clone
                                            media_type: mime_type.clone(),
                                            // rust-doctor-disable-next-line excessive-clone
                                            data: data.clone(),
                                        },
                                    }),
                                    _ => {}
                                }
                            }
                            let output = parts.join("\n");
                            // Sanitize tool_use_id
                            let sanitized_id: String = tool_call_id
                                .chars()
                                .map(|c| {
                                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                        c
                                    } else {
                                        '_'
                                    }
                                })
                                .take(64)
                                .collect();
                            tool_blocks.push(ContentBlock::ToolResult {
                                tool_use_id: sanitized_id,
                                content: output,
                                is_error: *is_error,
                            });
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    // Sibling image blocks ride after the tool_result blocks in
                    // the same user message.
                    tool_blocks.extend(image_blocks);
                    result.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal {
                            content: tool_blocks,
                        },
                    });
                }
            }
        }
        result
    }

    /// Build the comma-separated anthropic-beta header value for a given model.
    ///
    /// Each beta is gated on a capability bit so per-endpoint policy can opt
    /// out (e.g. `MiniMax` drops `fine-grained-tool-streaming`; Azure/Bedrock
    /// enable `context-1m`). Adds:
    /// - `interleaved-thinking-2025-05-14` — `caps.supports_interleaved_thinking`
    /// - `fine-grained-tool-streaming-2025-05-14` — `caps.supports_fine_grained_tool_streaming`
    /// - `output-128k-2025-02-19` — Claude 4 family (opus-4, sonnet-4)
    /// - `context-1m-2025-08-07` — `caps.supports_context_1m` AND Claude 4 family
    /// - OAuth stack (`claude-code-20250219` + `oauth-2025-04-20` + `token-restricted`)
    ///   when the API key is an Anthropic OAuth token — see `is_oauth_token`
    /// - `extended-cache-ttl-2025-04-11` when `extended_cache_ttl` is true (Long retention)
    pub(super) fn build_beta_headers(
        model: &str,
        api_key: Option<&str>,
        extended_cache_ttl: bool,
        caps: &AnthropicCapabilities,
    ) -> String {
        let mut betas: Vec<&'static str> = Vec::new();
        if caps.supports_interleaved_thinking {
            betas.push("interleaved-thinking-2025-05-14");
        }
        if caps.supports_fine_grained_tool_streaming {
            betas.push("fine-grained-tool-streaming-2025-05-14");
        }
        if Self::is_large_context_model(model) {
            betas.push("output-128k-2025-02-19");
        }
        // 1M context beta is only meaningful on Claude 4.x models; sending it
        // to older models is harmless but the gate keeps headers clean.
        if caps.supports_context_1m && Self::is_claude_4_family(model) {
            betas.push("context-1m-2025-08-07");
        }
        if api_key.is_some_and(Self::is_oauth_token) {
            // OAuth requests need the full Claude Code beta stack — without
            // claude-code/oauth Anthropic's OAuth infrastructure intermittently
            // 500s; without token-restricted the token's scope check fails.
            betas.push("claude-code-20250219");
            betas.push("oauth-2025-04-20");
            betas.push("token-restricted");
        }
        if extended_cache_ttl {
            betas.push("extended-cache-ttl-2025-04-11");
        }
        betas.join(",")
    }

    /// True for Claude 4-family models (opus-4-*, sonnet-4-*, haiku-4-*).
    /// Includes 4.5/4.6/4.7. Used to gate `context-1m-2025-08-07`.
    pub(super) fn is_claude_4_family(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("opus-4") || m.contains("sonnet-4") || m.contains("haiku-4")
    }

    /// Returns true for large context models that support 128k output tokens.
    pub(super) fn is_large_context_model(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("opus-4") || m.contains("sonnet-4")
    }

    /// Detect Anthropic-issued OAuth / setup tokens.
    ///
    /// Anthropic OAuth tokens authenticate via `Authorization: Bearer` and
    /// require the Claude Code beta stack + user-agent, while regular console
    /// API keys (`sk-ant-api*`) use `x-api-key`. Mis-routing either way produces
    /// a 401 / 403 at the API.
    ///
    /// Positively identified prefixes (mirrors hermes-agent `_is_oauth_token`):
    /// - `sk-ant-` but NOT `sk-ant-api` — Anthropic setup tokens / managed keys
    /// - `eyJ` — JWTs from the OAuth flow
    /// - `cc-` — Claude Code opaque OAuth access tokens
    ///
    /// Non-Anthropic keys (`MiniMax`, `DashScope`, Bedrock IAM, etc.) never match.
    pub(super) fn is_oauth_token(key: &str) -> bool {
        if key.is_empty() {
            return false;
        }
        if key.starts_with("sk-ant-api") {
            return false;
        }
        key.starts_with("sk-ant-") || key.starts_with("eyJ") || key.starts_with("cc-")
    }

    /// Map `ThinkLevel` to `budget_tokens`
    pub(super) const fn map_think_level(level: &ThinkLevel) -> Option<u32> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(1024),
            ThinkLevel::Low => Some(4096),
            ThinkLevel::Medium => Some(10000),
            ThinkLevel::High => Some(20000),
            ThinkLevel::XHigh => Some(50000),
        }
    }

    /// Extract a model's version as `(major, minor)` from its ID segments.
    ///
    /// Scans dash-separated segments (after lowercasing and dot→dash
    /// normalization, so `claude-opus-4.6` and Bedrock `us.anthropic.claude-…`
    /// both parse) and takes the **first run of consecutive numeric segments**
    /// as the version: `claude-opus-4-8` → `(4, 8)`, `claude-3-5-sonnet` →
    /// `(3, 5)`, `claude-fable-5` → `(5, 0)`.
    ///
    /// Only 1–2 digit segments count as version components — this skips
    /// date stamps (`claude-sonnet-4-5-20250929` → `(4, 5)`) and keeps
    /// non-Claude IDs routed through this protocol (e.g. `kimi-k2-0905`,
    /// `deepseek-v3-1`) from false-matching a modern Claude generation.
    ///
    /// The previous substring enumeration (`contains("-4-6") || …`) had to be
    /// re-edited for every model launch and silently mis-gated newer
    /// generations: 4.8 / 5.x fell through to the legacy `budget_tokens` +
    /// sampling-params path, both of which 400 on those models. A numeric
    /// compare passes the future-proof test — a newer model ID gates
    /// correctly with no code change.
    pub(super) fn claude_version(model: &str) -> Option<(u32, u32)> {
        let m = model.to_lowercase().replace('.', "-");
        let is_version_seg = |seg: &str| seg.len() <= 2 && seg.parse::<u32>().is_ok();
        let mut numbers = m
            .split('-')
            .skip_while(|seg| !is_version_seg(seg))
            .take_while(|seg| is_version_seg(seg))
            .map(|seg| seg.parse::<u32>().unwrap_or(0));
        let major = numbers.next()?;
        Some((major, numbers.next().unwrap_or(0)))
    }

    /// True for Claude 4.6+ models that support adaptive thinking
    /// (`thinking: { "type": "adaptive" }` + `output_config.effort`).
    ///
    /// Manual `budget_tokens` is deprecated on 4.6 and **removed** (400) on
    /// 4.7+ — Anthropic recommends adaptive so the model picks its own budget
    /// per turn. Version compare handles date-stamped variants like
    /// `claude-opus-4-6-20251110`, dot/hyphen normalization, and future
    /// generations (4.8, fable-5) without per-launch edits.
    pub(super) fn supports_adaptive_thinking(model: &str) -> bool {
        Self::claude_version(model).is_some_and(|v| v >= (4, 6))
    }

    /// True for Claude models that have *any* extended-thinking mode (3.7+).
    ///
    /// Claude 3.7 introduced extended thinking; every later generation keeps it
    /// (legacy `budget_tokens` on 3.7–4.5, adaptive on 4.6+). Below 3.7 —
    /// Claude 3.5 and 3.0 — there is **no** thinking mode, and an `{type:
    /// "enabled", budget_tokens}` block returns a 400. The old code emitted that
    /// block for any non-adaptive model whenever a think level was set, so a
    /// cheap non-reasoning model (Haiku 3/3.5) carrying a configured think level
    /// hard-failed the request. This gates the legacy arm so the think level is
    /// simply ignored there instead.
    ///
    /// A non-Claude model proxied through this protocol (version unknown) is
    /// treated as thinking-capable — **fail-open**, to avoid regressing custom
    /// reasoning endpoints; the OpenAI-compat `supports_reasoning_effort` strip
    /// is the gate for those wires. Version compare keeps this future-proof: a
    /// newer generation needs no edit here (same rationale as [`claude_version`]).
    pub(super) fn supports_extended_thinking(model: &str) -> bool {
        Self::claude_version(model).is_none_or(|v| v >= (3, 7))
    }

    /// True for models that 400 on non-default `temperature/top_p/top_k` even
    /// without `thinking` enabled. Claude 4.7 removed sampling parameters and
    /// every later release (4.8, fable-5) keeps that surface.
    pub(super) fn forbids_sampling_params(model: &str) -> bool {
        Self::claude_version(model).is_some_and(|v| v >= (4, 7))
    }

    /// True for models that accept the `xhigh` adaptive effort level (Claude
    /// 4.7+). 4.6 models reject `xhigh` with a 400 — callers must downgrade
    /// to `max` when this returns false.
    pub(super) fn supports_xhigh_effort(model: &str) -> bool {
        Self::claude_version(model).is_some_and(|v| v >= (4, 7))
    }

    /// True for generation-5 models (`claude-fable-5`, …) where an explicit
    /// `thinking: {type: "disabled"}` block returns a 400 — the only way to
    /// run without thinking is to **omit** the `thinking` field entirely.
    /// 4.6–4.8 still accept (and need) the explicit disabled block to
    /// suppress their default thinking, so this gates only 5.x+.
    pub(super) fn omits_disabled_thinking(model: &str) -> bool {
        Self::claude_version(model).is_some_and(|v| v >= (5, 0))
    }

    /// Map [`ThinkLevel`] to an Anthropic adaptive-thinking `effort` string.
    ///
    /// Anthropic 4.7+ exposes five levels (`low`, `medium`, `high`, `xhigh`,
    /// `max`); 4.6 only exposes four (`low`, `medium`, `high`, `max`). When
    /// `xhigh` would be selected on a 4.6 model we downgrade to `max`
    /// (the strongest level it accepts) per hermes-agent
    /// `ADAPTIVE_EFFORT_MAP` + `_supports_xhigh_effort`.
    ///
    /// `ThinkLevel::Off` is rejected at the call site (adaptive thinking
    /// requires *some* effort), so this returns `None` for `Off`.
    pub(super) fn map_think_level_to_adaptive_effort(
        level: &ThinkLevel,
        model: &str,
    ) -> Option<&'static str> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal | ThinkLevel::Low => Some("low"),
            ThinkLevel::Medium => Some("medium"),
            ThinkLevel::High => Some("high"),
            ThinkLevel::XHigh => {
                if Self::supports_xhigh_effort(model) {
                    Some("xhigh")
                } else {
                    Some("max")
                }
            }
        }
    }

    /// Prepend the mandatory Claude Code identity block to an OAuth request's
    /// `system` array.
    ///
    /// Anthropic OAuth requests must lead with [`CLAUDE_CODE_IDENTITY`] as the
    /// first `system` block or the API rejects them (401/403). The identity is
    /// inserted *before* any caller-supplied blocks (including the cache-first
    /// stable block), so it joins the cacheable prefix without disturbing the
    /// existing `cache_control` marker placement: the breakpoint stays on the
    /// stable block, and everything before that marker — the identity included
    /// — is cached by Anthropic's prefix semantics.
    ///
    /// `None` (no caller system prompt) collapses to a single-element array
    /// carrying only the identity, which is exactly what bare OAuth requests
    /// need.
    pub(super) fn prepend_claude_code_identity(
        system: Option<Vec<SystemBlock>>,
    ) -> Vec<SystemBlock> {
        let mut blocks = Vec::with_capacity(system.as_ref().map_or(1, |s| s.len() + 1));
        blocks.push(SystemBlock::text(CLAUDE_CODE_IDENTITY));
        if let Some(existing) = system {
            blocks.extend(existing);
        }
        blocks
    }

    /// Guarantee `max_tokens > thinking.budget_tokens` for legacy (non-adaptive)
    /// extended thinking.
    ///
    /// The legacy `{type: "enabled", budget_tokens: N}` thinking config carves
    /// its budget out of `max_tokens`; Anthropic 400s when the budget meets or
    /// exceeds `max_tokens`. With the default 16k cap this bites every
    /// `High` (20k) / `XHigh` (50k) legacy-thinking turn. Mirror openclaw
    /// `adjustMaxTokensForThinking`: raise `max_tokens` to `budget +
    /// MIN_OUTPUT_TOKENS_WITH_THINKING` so a visible answer still fits.
    ///
    /// Adaptive thinking (4.6/4.7) carries `budget_tokens: None` and is left
    /// untouched — those models size their own budget per turn.
    pub(super) fn adjust_max_tokens_for_thinking_budget(
        max_tokens: u32,
        thinking: Option<&ThinkingBlock>,
    ) -> u32 {
        match thinking.and_then(|t| t.budget_tokens) {
            Some(budget) if budget >= max_tokens => {
                budget.saturating_add(MIN_OUTPUT_TOKENS_WITH_THINKING)
            }
            _ => max_tokens,
        }
    }
}
