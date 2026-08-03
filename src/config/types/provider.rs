//! Provider configuration types
//!
//! Contains AI provider configuration:
//! - `ProviderConfig`: Individual provider settings (API key, model, etc.)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// CacheRetention
// =============================================================================

/// Prompt cache retention policy for streaming protocols that support it.
///
/// - `Off`: never inject `cache_control` breakpoints.
/// - `Short` (default): provider default retention (Anthropic: 5-minute
///   ephemeral cache).
/// - `Long`: extended retention. Anthropic → 1-hour ephemeral cache plus the
///   `anthropic-beta: extended-cache-ttl-2025-04-11` header; official
///   `OpenAI` → `prompt_cache_retention: "24h"` on Chat/Responses requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    Off,
    #[default]
    Short,
    Long,
}

// =============================================================================
// ResponseFormat
// =============================================================================

/// Structured output format for the model response.
///
/// Maps to:
/// - Chat protocol → top-level `response_format` field
/// - Responses protocol → `text.format` field (already typed as `TextFormat`)
///
/// Capability-gated: silently dropped when endpoint doesn't support it
/// (see `ProviderCapabilities::supports_response_format`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Free-form text (default; equivalent to no field).
    Text,
    /// Force valid JSON output (no schema).
    JsonObject,
    /// Force JSON matching the provided schema. Strict mode enabled when
    /// endpoint supports it.
    JsonSchema {
        name: String,
        schema: serde_json::Value,
    },
}

// =============================================================================
// ProviderConfig
// =============================================================================

/// AI Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProviderConfig {
    /// Protocol to use: "openai", "anthropic", "gemini", "ollama"
    /// If not specified, defaults to "openai"
    #[serde(default)]
    pub protocol: Option<String>,
    /// Runtime-only API key (populated from encrypted vault, never persisted to config.toml)
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,
    /// Model names (e.g., ["gpt-4o", "gpt-4o-mini"]). First model is the default.
    /// Accepts both `model = "xxx"` (backward compat) and `models = ["xxx", ...]`.
    #[serde(
        deserialize_with = "crate::config::types::serde_helpers::deserialize_models",
        alias = "model"
    )]
    pub models: Vec<String>,
    /// Base URL for API endpoint (optional, defaults to official API)
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider brand color for UI (hex string, e.g., "#10a37f")
    #[serde(default = "default_provider_color")]
    pub color: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Idle timeout for streaming responses, in seconds — the maximum gap with
    /// no bytes from the upstream.
    ///
    /// Two watchdogs honor it: (1) the time-to-first-byte guard in
    /// `HttpProvider::execute` aborts if the upstream returns no response
    /// headers within this duration (covers a provider that accepts the
    /// connection but stalls before responding); (2) the Anthropic protocol
    /// adapter wraps each SSE event read so a stall *between* events also
    /// aborts. Both surface `AlephError::Timeout`, which the failover path
    /// treats as transient. Distinct from `timeout_seconds` (total request
    /// budget, not applied to the streaming path).
    ///
    /// `None` or unset: 60 seconds (default).
    /// `Some(0)`: both watchdogs disabled.
    ///
    /// The TTFB guard applies to every HTTP protocol; the inter-event guard is
    /// currently honored only by the Anthropic protocol adapter.
    #[serde(default)]
    pub stream_idle_timeout_secs: Option<u64>,
    /// Prompt cache retention policy. See [`CacheRetention`] for the
    /// per-protocol mapping — it is honored on the Anthropic protocol (1h
    /// `ttl` on the official endpoint) *and* on official `OpenAI` endpoints
    /// (`prompt_cache_retention: "24h"` on Chat and Responses).
    ///
    /// `None` (unset) means "provider default retention", NOT "off": the
    /// Anthropic adapter resolves it to `Short` (the standard 5-minute
    /// ephemeral marker) and `OpenAI` simply omits the retention field, which
    /// leaves its implicit prefix caching on.
    ///
    /// Whether markers may be emitted at all is a separate, endpoint-level
    /// decision: `policy.capabilities.supports_cache_control`, which is false
    /// by default for `Custom`-class hosts and opted back in per family
    /// (Bedrock, Azure). An endpoint without that capability is treated
    /// exactly like `Off`.
    ///
    /// An explicit value (`Off` / `Short` / `Long`) is always respected, and
    /// `Off` means what it says: no `cache_control` anywhere on the request,
    /// pre-placed markers stripped included.
    #[serde(default)]
    pub cache_retention: Option<CacheRetention>,
    /// Whether the provider is enabled/active
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,

    // Common generation parameters
    /// Maximum tokens in response (optional)
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Operator-declared total context window (tokens) for this provider's
    /// model(s). Escape hatch for third-party endpoints (302ai / moonshot /
    /// openrouter) whose effective window differs from — or is absent in — the
    /// static capability catalog (`capabilities_for`). Consumed only by
    /// model-aware context-compaction budgeting: when unset, the catalog (then a
    /// built-in default) is used. Has no effect on the wire request.
    #[serde(default)]
    pub context_window: Option<u32>,
    /// Temperature for response randomness (0.0-2.0 for OpenAI/Gemini, 0.0-1.0 for Claude)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling (0.0-1.0, optional)
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Top-k sampling (integer, optional, used by Claude, Gemini, Ollama)
    #[serde(default)]
    pub top_k: Option<u32>,

    // OpenAI-specific parameters
    /// Frequency penalty (-2.0 to 2.0, `OpenAI` only)
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0, `OpenAI` only)
    #[serde(default)]
    pub presence_penalty: Option<f32>,

    // Claude/Gemini/Ollama-specific parameters
    /// Stop sequences (comma-separated, Claude/Gemini/Ollama)
    #[serde(default)]
    pub stop_sequences: Option<String>,

    // Gemini-specific parameters
    /// Thinking level for Gemini 3 models (LOW or HIGH)
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// Media resolution for Gemini (LOW, MEDIUM, HIGH)
    #[serde(default)]
    pub media_resolution: Option<String>,

    // Ollama-specific parameters
    /// Repeat penalty for Ollama (default 1.1)
    #[serde(default)]
    pub repeat_penalty: Option<f32>,

    // System prompt handling mode
    /// How to send system prompts to the API:
    /// - "prepend" (default): Prepend system prompt to user message (for APIs that ignore system role)
    /// - "standard": Use a separate system message (for standard OpenAI-compatible APIs)
    #[serde(default)]
    pub system_prompt_mode: Option<String>,

    /// Model behavior override: use a specific behavior file instead of protocol default.
    /// Maps to a file in `~/.aleph/model_behaviors/{name}.md` or a built-in behavior.
    /// Example: Set to "anthropic" on an `OpenRouter` provider that routes to Claude.
    #[serde(default)]
    pub model_behavior: Option<String>,

    /// Whether this provider has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,

    /// Latency/cost service tier. Capability-gated per endpoint and shared
    /// across protocols: Anthropic accepts "`auto"/"standard_only`"; `OpenAI`
    /// (chat + responses) accepts "auto"/"default"/"flex"/"priority".
    #[serde(default)]
    pub service_tier: Option<String>,

    // OpenAI Cycle 2 fields
    /// Structured output format (None = free-form text).
    /// Capability-gated; silently dropped when endpoint doesn't support it.
    #[serde(default)]
    pub response_format: Option<ResponseFormat>,

    /// Whether the model accepts parallel tool calls (None = server default).
    /// When None, no `parallel_tool_calls` field is sent.
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,

    /// Deterministic sampling seed. None = server default.
    /// Capability-gated; silently dropped when endpoint doesn't support it.
    #[serde(default)]
    pub seed: Option<u64>,

    /// Whether to return per-token logprobs. None = no field emitted.
    /// Capability-gated; silently dropped when endpoint doesn't support it.
    #[serde(default)]
    pub logprobs: Option<bool>,

    /// Number of top alternative tokens per position (Chat range: 0..=20).
    /// Only emitted when `logprobs = Some(true)` and capability supports it.
    #[serde(default)]
    pub top_logprobs: Option<u8>,

    // Anthropic-specific parameters (Cycle 4)
    /// End-user identifier passed in `metadata.user_id` of Anthropic requests.
    /// Helps Anthropic detect and prevent abuse. None = field omitted.
    #[serde(default)]
    pub metadata_user_id: Option<String>,

    /// Extended thinking budget token hint ("low" | "medium" | "high" | numeric string).
    /// Mapped to `thinking.budget_tokens` when the Anthropic adapter builds the request.
    /// None = field omitted (thinking disabled).
    #[serde(default)]
    pub effort: Option<String>,
}

pub fn default_provider_color() -> String {
    "#808080".to_string() // Gray as default
}

pub fn default_timeout_seconds() -> u64 {
    crate::config::defaults_override::get_defaults_override()
        .provider_timeout_seconds()
        .unwrap_or(300)
}

pub const fn default_provider_enabled() -> bool {
    false // Providers are disabled by default, user must explicitly enable them
}

impl ProviderConfig {
    /// Get the effective protocol name
    ///
    /// Returns the explicit protocol if set, otherwise defaults to "openai".
    /// WARNING: Callers should ensure protocol is explicitly set before calling this.
    /// The "openai" default is a legacy fallback — see Task 2 for persistence fix.
    #[must_use]
    pub fn protocol(&self) -> String {
        self.protocol
            .clone()
            .unwrap_or_else(|| "openai".to_string())
    }

    /// Returns the default model (first in the list).
    ///
    /// If the first entry is a comma-separated string of fallback models
    /// (e.g. "gpt-5.4,gpt-5.3-codex"), returns only the first model name.
    #[must_use]
    pub fn default_model(&self) -> &str {
        let first = self.models.first().map_or("", |s| s.as_str());
        // Handle comma-separated model lists stored as a single string
        first.split(',').next().unwrap_or(first).trim()
    }

    /// Returns all configured models
    #[must_use]
    pub fn all_models(&self) -> &[String] {
        &self.models
    }

    /// Create a minimal test configuration with only required fields
    ///
    /// This is a helper for tests to avoid specifying all optional fields.
    /// All optional advanced parameters (like `frequency_penalty`, `media_resolution`, etc.) are set to None.
    pub fn test_config(model: impl Into<String>) -> Self {
        Self {
            protocol: None,
            api_key: Some("test-key".to_string()),
            models: vec![model.into()],
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: true, // Tests need enabled providers
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        assert_eq!(config.protocol(), "openai");
    }

    #[test]
    fn test_protocol_explicit() {
        let mut config = ProviderConfig::test_config("model");
        config.protocol = Some("anthropic".to_string());
        assert_eq!(config.protocol(), "anthropic");
    }

    #[test]
    fn test_protocol_without_provider_type() {
        let config = ProviderConfig {
            protocol: Some("anthropic".to_string()),
            models: vec!["claude-3-5-sonnet".to_string()],
            api_key: None,
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: default_provider_enabled(),
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
        };
        assert_eq!(config.protocol(), "anthropic");
    }

    #[test]
    fn test_protocol_defaults_to_openai() {
        let config = ProviderConfig {
            protocol: None,
            models: vec!["gpt-4".to_string()],
            api_key: None,
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: default_provider_enabled(),
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
        };
        assert_eq!(config.protocol(), "openai");
    }

    #[test]
    fn new_cycle3_fields_default_to_none() {
        let config = ProviderConfig::test_config("gpt-4o");
        assert!(config.seed.is_none());
        assert!(config.logprobs.is_none());
        assert!(config.top_logprobs.is_none());
    }

    #[test]
    fn cycle3_fields_deserialize_from_toml() {
        let toml_str = r#"
            protocol = "openai"
            models = ["gpt-4o"]
            seed = 42
            logprobs = true
            top_logprobs = 5
        "#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
        assert_eq!(cfg.seed, Some(42));
        assert_eq!(cfg.logprobs, Some(true));
        assert_eq!(cfg.top_logprobs, Some(5));
    }

    #[test]
    fn cycle3_fields_default_when_toml_omits_them() {
        let toml_str = r#"
            protocol = "openai"
            models = ["gpt-4o"]
        "#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
        assert!(cfg.seed.is_none());
        assert!(cfg.logprobs.is_none());
        assert!(cfg.top_logprobs.is_none());
    }

    #[test]
    fn cycle4_anthropic_fields_default_to_none() {
        let config = ProviderConfig::test_config("claude-opus-4-5");
        assert!(config.metadata_user_id.is_none());
        assert!(config.effort.is_none());
    }

    #[test]
    fn cycle4_anthropic_fields_deserialize_from_toml() {
        let toml_str = r#"
            protocol = "anthropic"
            models = ["claude-opus-4-5"]
            metadata_user_id = "user-abc123"
            effort = "high"
        "#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
        assert_eq!(cfg.metadata_user_id, Some("user-abc123".to_string()));
        assert_eq!(cfg.effort, Some("high".to_string()));
    }

    #[test]
    fn cycle4_anthropic_fields_omit_in_toml_yields_none() {
        let toml_str = r#"
            protocol = "anthropic"
            models = ["claude-opus-4-5"]
        "#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
        assert!(cfg.metadata_user_id.is_none());
        assert!(cfg.effort.is_none());
    }
}
