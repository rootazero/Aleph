//! Provider-specific payload policy for OpenAI-compatible protocols.
//!
//! Detects endpoint provider class from base URL, then applies per-provider
//! field filtering/injection for `OpenAI` Chat and Responses APIs.

use std::collections::HashSet;

// =============================================================================
// EndpointClass
// =============================================================================

/// Detected provider endpoint class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointClass {
    /// Official `OpenAI` API (api.openai.com)
    OpenAiPublic,
    /// `OpenAI` Codex (chatgpt.com)
    OpenAiCodex,
    /// Azure `OpenAI` Service (*.openai.azure.com)
    AzureOpenAi,
    /// Anthropic public API (api.anthropic.com)
    AnthropicPublic,
    /// `DeepSeek` native API (api.deepseek.com)
    DeepSeekNative,
    /// Groq native API (api.groq.com)
    GroqNative,
    /// Mistral public API (api.mistral.ai)
    MistralPublic,
    /// Moonshot native API (api.moonshot.ai / api.moonshot.cn)
    MoonshotNative,
    /// Cerebras native API (api.cerebras.ai)
    CerebrasNative,
    /// X.AI / Grok native API (api.x.ai / api.grok.x.ai)
    XAiNative,
    /// `OpenRouter` (openrouter.ai)
    OpenRouter,
    /// Local endpoint (localhost / 127.0.0.1 / *.local)
    Local,
    /// Unknown / custom endpoint
    Custom,
}

// =============================================================================
// ProviderCapabilities
// =============================================================================

/// Per-provider capability flags.
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    /// Supports `store` field in Responses API
    pub supports_responses_store: bool,
    /// Supports `reasoning_effort` field
    pub supports_reasoning_effort: bool,
    /// Supports `prompt_cache_key` / `prompt_cache_retention`
    pub supports_prompt_cache: bool,
    /// Supports `service_tier` field
    pub supports_service_tier: bool,
    /// Supports strict JSON schema mode
    pub supports_strict_schema: bool,
    /// Supports `response_format` (Chat) / `text.format` (Responses) field
    pub supports_response_format: bool,
    /// Endpoint accepts the `seed` field on Chat/Responses requests
    pub supports_seed: bool,
    /// Endpoint accepts `logprobs` / `top_logprobs` on Chat/Responses requests
    pub supports_logprobs: bool,
    /// Supports server-side context compaction
    pub supports_server_compaction: bool,
    /// Known to reject object schemas without `properties`
    pub requires_object_properties: bool,
    /// Known to mis-handle local `$ref` nodes in tool parameter schemas
    /// (Moonshot reports "infinite recursion" or "type is not defined").
    pub requires_derefed_refs: bool,
    /// Maximum context window (for compaction threshold calculation)
    pub context_window: Option<usize>,
}

// =============================================================================
// PayloadPolicy
// =============================================================================

/// Resolved payload policy for a specific provider + API combination.
#[derive(Debug, Clone)]
pub struct PayloadPolicy {
    /// Detected endpoint class
    pub endpoint_class: EndpointClass,
    /// Provider capabilities
    pub capabilities: ProviderCapabilities,
    /// Explicit store value override (None = don't set)
    pub explicit_store: Option<bool>,
    /// Whether to strip `store` from payload
    pub strip_store: bool,
    /// Whether to strip reasoning-related fields
    pub strip_reasoning: bool,
    /// Whether to strip prompt cache fields
    pub strip_prompt_cache: bool,
    /// Server compaction threshold (None = disable)
    pub compaction_threshold: Option<usize>,
}

impl PayloadPolicy {
    /// Apply policy to a request payload object (mutates in place).
    pub fn apply(&self, payload: &mut serde_json::Map<String, serde_json::Value>) {
        // Store field
        if let Some(store) = self.explicit_store {
            payload.insert("store".into(), serde_json::Value::Bool(store));
        } else if self.strip_store {
            payload.remove("store");
        }

        // Reasoning field
        if self.strip_reasoning {
            payload.remove("reasoning");
            payload.remove("reasoning_effort");
        }

        // Response format (when capability disabled)
        if !self.capabilities.supports_response_format {
            payload.remove("response_format");
        }

        // Seed (when capability disabled)
        if !self.capabilities.supports_seed {
            payload.remove("seed");
        }

        // Logprobs (when capability disabled)
        if !self.capabilities.supports_logprobs {
            payload.remove("logprobs");
            payload.remove("top_logprobs");
        }

        // Prompt cache fields
        if self.strip_prompt_cache {
            payload.remove("prompt_cache_key");
            payload.remove("prompt_cache_retention");
        }

        // Service tier
        if !self.capabilities.supports_service_tier {
            payload.remove("service_tier");
        }

        // Server compaction
        if let Some(threshold) = self.compaction_threshold {
            if payload.get("context_management").is_none() {
                payload.insert(
                    "context_management".into(),
                    serde_json::json!([{
                        "type": "compaction",
                        "compact_threshold": threshold
                    }]),
                );
            }
        }
    }

    /// Apply policy to a tool schema (mutates in place).
    pub fn apply_to_schema(&self, schema: &mut serde_json::Value) {
        if self.capabilities.requires_object_properties {
            crate::providers::protocols::openai_common::tools::ensure_properties_recursive(schema);
        }
        if self.capabilities.requires_derefed_refs {
            crate::providers::protocols::openai_common::openai_strict_schema::deref_json_schema(
                schema,
            );
            crate::providers::protocols::openai_common::openai_strict_schema::ensure_property_types(
                schema,
            );
        }
    }
}

// =============================================================================
// Endpoint Detection
// =============================================================================

/// Detect endpoint class from base URL.
#[must_use]
pub fn detect_endpoint_class(base_url: Option<&str>) -> EndpointClass {
    let url = match base_url {
        None | Some("") => return EndpointClass::OpenAiPublic,
        Some(u) => u,
    };

    let host = match extract_hostname(url) {
        Some(h) => h.to_lowercase(),
        None => return EndpointClass::Custom,
    };

    match host.as_str() {
        "api.openai.com" => EndpointClass::OpenAiPublic,
        "chatgpt.com" => EndpointClass::OpenAiCodex,
        "api.anthropic.com" => EndpointClass::AnthropicPublic,
        "api.deepseek.com" => EndpointClass::DeepSeekNative,
        "api.groq.com" => EndpointClass::GroqNative,
        "api.mistral.ai" => EndpointClass::MistralPublic,
        "api.moonshot.ai" | "api.moonshot.cn" | "api.kimi.com" => EndpointClass::MoonshotNative,
        "api.cerebras.ai" => EndpointClass::CerebrasNative,
        "api.x.ai" | "api.grok.x.ai" => EndpointClass::XAiNative,
        _ => {
            if host.ends_with(".openai.azure.com") {
                EndpointClass::AzureOpenAi
            } else if host.ends_with("openrouter.ai") {
                EndpointClass::OpenRouter
            } else if is_local_host(&host) {
                EndpointClass::Local
            } else {
                EndpointClass::Custom
            }
        }
    }
}

fn extract_hostname(url: &str) -> Option<String> {
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };

    with_scheme
        .parse::<url::Url>()
        .ok()
        .map(|u| u.host_str().unwrap_or("").to_string())
}

fn is_local_host(host: &str) -> bool {
    let local_hosts: HashSet<&str> = ["localhost", "127.0.0.1", "::1", "[::1]"]
        .iter()
        .cloned()
        .collect();

    local_hosts.contains(host) || host.ends_with(".localhost") || host.ends_with(".local")
}

// =============================================================================
// Capability Resolution
// =============================================================================

/// Resolve capabilities for a given endpoint class.
#[must_use]
pub const fn resolve_capabilities(class: EndpointClass) -> ProviderCapabilities {
    match class {
        EndpointClass::OpenAiPublic => ProviderCapabilities {
            supports_responses_store: true,
            supports_reasoning_effort: true,
            supports_prompt_cache: true,
            supports_service_tier: true,
            supports_strict_schema: true,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: true,
            requires_object_properties: false,
            requires_derefed_refs: false,
            context_window: Some(128_000),
        },
        EndpointClass::OpenAiCodex => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: true,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: true,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: Some(128_000),
        },
        EndpointClass::AzureOpenAi => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: true,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: None,
        },
        EndpointClass::AnthropicPublic => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: true,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_seed: false,
            supports_logprobs: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: Some(200_000),
        },
        EndpointClass::DeepSeekNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            // deepseek-chat / deepseek-reasoner now map to deepseek-v4-flash
            // (non-thinking / thinking modes), whose advertised context length
            // is 1M tokens — the legacy 64K reflected the V3-era window. Source:
            // https://api-docs.deepseek.com/quick_start/pricing
            context_window: Some(1_000_000),
        },
        EndpointClass::GroqNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: Some(8_000),
        },
        EndpointClass::MistralPublic => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: true,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: Some(128_000),
        },
        // Moonshot / Kimi. `supports_reasoning_effort` answers "does this
        // endpoint understand the field", and since K3 it does — the flag was
        // written when no Kimi model took an effort, and left K3's headline
        // knob stripped on the wire: the user's think level vanished and every
        // request ran the vendor default.
        //
        // Which *models* accept it is a separate, finer question, answered by
        // `reasoning_effort::supported_efforts` — which returns an empty set
        // for every non-K3 Kimi id (including ids it has never seen), so the
        // field is still never sent to a model that would 400 on it.
        EndpointClass::MoonshotNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: true,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: true,
            context_window: Some(128_000),
        },
        EndpointClass::CerebrasNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: Some(128_000),
        },
        EndpointClass::XAiNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: Some(128_000),
        },
        EndpointClass::OpenRouter => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: true,
            supports_seed: true,
            supports_logprobs: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: None,
        },
        EndpointClass::Local => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_seed: false,
            supports_logprobs: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: None,
        },
        EndpointClass::Custom => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_seed: false,
            supports_logprobs: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            requires_derefed_refs: false,
            context_window: None,
        },
    }
}

// =============================================================================
// Policy Builder
// =============================================================================

/// Build a payload policy from provider configuration.
#[must_use]
pub fn build_payload_policy(
    base_url: Option<&str>,
    api_type: &str,
    variant_store: Option<bool>,
) -> PayloadPolicy {
    let class = detect_endpoint_class(base_url);
    let capabilities = resolve_capabilities(class);

    let is_responses_api = api_type == "openai-responses" || api_type == "codex-responses";

    // Determine store policy
    let (explicit_store, strip_store) = if let Some(forced) = variant_store {
        (Some(forced), false)
    } else if is_responses_api && capabilities.supports_responses_store {
        (Some(true), false)
    } else if is_responses_api {
        (None, true)
    } else {
        // Chat Completions has no `store` field — it is Responses-API-only.
        // Strip it defensively so a stray value never reaches a chat endpoint.
        (None, true)
    };

    let strip_reasoning = !capabilities.supports_reasoning_effort;
    let strip_prompt_cache = !capabilities.supports_prompt_cache;

    let compaction_threshold = if capabilities.supports_server_compaction {
        capabilities.context_window.map(|cw| cw * 7 / 10)
    } else {
        None
    };

    PayloadPolicy {
        endpoint_class: class,
        capabilities,
        explicit_store,
        strip_store,
        strip_reasoning,
        strip_prompt_cache,
        compaction_threshold,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_openai_public() {
        assert_eq!(
            detect_endpoint_class(Some("https://api.openai.com/v1")),
            EndpointClass::OpenAiPublic
        );
    }

    #[test]
    fn test_detect_deepseek() {
        assert_eq!(
            detect_endpoint_class(Some("https://api.deepseek.com")),
            EndpointClass::DeepSeekNative
        );
    }

    #[test]
    fn test_detect_localhost() {
        assert_eq!(
            detect_endpoint_class(Some("http://localhost:8080")),
            EndpointClass::Local
        );
    }

    #[test]
    fn test_openai_capabilities() {
        let caps = resolve_capabilities(EndpointClass::OpenAiPublic);
        assert!(caps.supports_responses_store);
        assert!(caps.supports_reasoning_effort);
        assert!(caps.supports_strict_schema);
        assert!(caps.supports_server_compaction);
    }

    #[test]
    fn test_deepseek_capabilities() {
        let caps = resolve_capabilities(EndpointClass::DeepSeekNative);
        assert!(!caps.supports_responses_store);
        assert!(!caps.supports_reasoning_effort);
        assert!(!caps.supports_strict_schema);
        assert!(!caps.supports_server_compaction);
        // Current deepseek-chat/reasoner (→ deepseek-v4-flash) advertise a 1M
        // context window; guard against silently regressing to the stale 64K.
        assert_eq!(caps.context_window, Some(1_000_000));
    }

    #[test]
    fn test_build_policy_for_openai() {
        let policy = build_payload_policy(Some("https://api.openai.com"), "openai-responses", None);
        assert_eq!(policy.endpoint_class, EndpointClass::OpenAiPublic);
        assert_eq!(policy.explicit_store, Some(true));
        assert!(!policy.strip_reasoning);
        assert_eq!(policy.compaction_threshold, Some(89_600));
    }

    #[test]
    fn test_build_policy_for_deepseek() {
        let policy =
            build_payload_policy(Some("https://api.deepseek.com"), "openai-responses", None);
        assert_eq!(policy.endpoint_class, EndpointClass::DeepSeekNative);
        assert!(policy.strip_store);
        assert!(policy.strip_reasoning);
        assert!(policy.compaction_threshold.is_none());
    }

    #[test]
    fn test_apply_policy_strips_fields() {
        let policy = build_payload_policy(Some("https://api.deepseek.com"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert("store".into(), serde_json::Value::Bool(true));
        payload.insert("reasoning".into(), serde_json::json!({"effort": "high"}));
        payload.insert("model".into(), serde_json::Value::String("test".into()));

        policy.apply(&mut payload);

        assert!(payload.get("store").is_none());
        assert!(payload.get("reasoning").is_none());
        assert!(payload.get("model").is_some()); // should not be stripped
    }

    #[test]
    fn test_apply_policy_adds_compaction() {
        let policy = build_payload_policy(Some("https://api.openai.com"), "openai-responses", None);
        let mut payload = serde_json::Map::new();
        policy.apply(&mut payload);

        assert!(payload.get("context_management").is_some());
        assert!(payload.get("store").is_some());
    }

    #[test]
    fn test_detect_azure() {
        assert_eq!(
            detect_endpoint_class(Some("https://my-resource.openai.azure.com")),
            EndpointClass::AzureOpenAi
        );
    }

    #[test]
    fn kimi_endpoint_requires_derefed_refs_and_object_properties() {
        let caps = resolve_capabilities(EndpointClass::MoonshotNative);
        assert!(caps.requires_derefed_refs);
        assert!(caps.requires_object_properties);
    }

    /// The endpoint understands `reasoning_effort` since K3, so the blanket
    /// strip must be off — it was silently discarding the one control K3
    /// exposes. Per-model admission is `reasoning_effort::supported_efforts`.
    #[test]
    fn kimi_endpoint_no_longer_strips_reasoning_effort() {
        let policy =
            build_payload_policy(Some("https://api.kimi.com/coding/v1"), "openai-chat", None);
        assert_eq!(policy.endpoint_class, EndpointClass::MoonshotNative);
        assert!(!policy.strip_reasoning);

        let mut payload = serde_json::Map::new();
        payload.insert(
            "reasoning_effort".into(),
            serde_json::Value::String("max".into()),
        );
        policy.apply(&mut payload);
        assert_eq!(
            payload.get("reasoning_effort").and_then(|v| v.as_str()),
            Some("max"),
            "the effort the adapter clamped must survive to the wire"
        );

        // Same for the open platform, which is the same endpoint class.
        let open = build_payload_policy(Some("https://api.moonshot.ai/v1"), "openai-chat", None);
        assert!(!open.strip_reasoning);
    }

    #[test]
    fn test_detect_kimi() {
        assert_eq!(
            detect_endpoint_class(Some("https://api.kimi.com/coding/v1")),
            EndpointClass::MoonshotNative
        );
    }

    #[test]
    fn test_detect_openrouter() {
        assert_eq!(
            detect_endpoint_class(Some("https://openrouter.ai/api/v1")),
            EndpointClass::OpenRouter
        );
    }

    #[test]
    fn test_detect_default_is_openai() {
        assert_eq!(detect_endpoint_class(None), EndpointClass::OpenAiPublic);
        assert_eq!(detect_endpoint_class(Some("")), EndpointClass::OpenAiPublic);
    }

    #[test]
    fn test_variant_store_override() {
        let policy = build_payload_policy(
            Some("https://api.openai.com"),
            "openai-responses",
            Some(false),
        );
        assert_eq!(policy.explicit_store, Some(false));
        assert!(!policy.strip_store);
    }

    #[test]
    fn test_custom_endpoint() {
        assert_eq!(
            detect_endpoint_class(Some("https://example.com")),
            EndpointClass::Custom
        );
    }

    #[test]
    fn openai_public_supports_response_format() {
        let caps = resolve_capabilities(EndpointClass::OpenAiPublic);
        assert!(caps.supports_response_format);
    }

    #[test]
    fn openai_codex_supports_response_format() {
        let caps = resolve_capabilities(EndpointClass::OpenAiCodex);
        assert!(caps.supports_response_format);
    }

    #[test]
    fn apply_strips_response_format_when_unsupported() {
        let policy = build_payload_policy(Some("http://localhost:8080"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert(
            "response_format".into(),
            serde_json::json!({"type": "json_object"}),
        );
        payload.insert("model".into(), serde_json::Value::String("local".into()));

        policy.apply(&mut payload);

        assert!(payload.get("response_format").is_none());
        assert!(payload.get("model").is_some());
    }

    #[test]
    fn apply_keeps_response_format_when_supported() {
        let policy = build_payload_policy(Some("https://api.openai.com"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert(
            "response_format".into(),
            serde_json::json!({"type": "json_object"}),
        );

        policy.apply(&mut payload);

        assert!(payload.get("response_format").is_some());
    }

    #[test]
    fn supports_seed_matrix_matches_cycle3_spec() {
        let truthy = [
            EndpointClass::OpenAiPublic,
            EndpointClass::OpenAiCodex,
            EndpointClass::AzureOpenAi,
            EndpointClass::OpenRouter,
            EndpointClass::DeepSeekNative,
            EndpointClass::GroqNative,
            EndpointClass::MistralPublic,
            EndpointClass::MoonshotNative,
            EndpointClass::CerebrasNative,
            EndpointClass::XAiNative,
        ];
        for class in truthy {
            assert!(
                resolve_capabilities(class).supports_seed,
                "{:?} should support seed",
                class
            );
        }
        let falsy = [
            EndpointClass::AnthropicPublic,
            EndpointClass::Local,
            EndpointClass::Custom,
        ];
        for class in falsy {
            assert!(
                !resolve_capabilities(class).supports_seed,
                "{:?} should NOT support seed",
                class
            );
        }
    }

    #[test]
    fn apply_strips_seed_when_unsupported() {
        let policy = build_payload_policy(Some("http://localhost:8080"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert("seed".into(), serde_json::json!(42));
        payload.insert("model".into(), serde_json::Value::String("m".into()));

        policy.apply(&mut payload);

        assert!(payload.get("seed").is_none());
        assert!(payload.get("model").is_some());
    }

    #[test]
    fn apply_keeps_seed_when_supported() {
        let policy = build_payload_policy(Some("https://api.openai.com"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert("seed".into(), serde_json::json!(42));

        policy.apply(&mut payload);

        assert_eq!(payload.get("seed"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn supports_logprobs_matrix_matches_cycle3_spec() {
        let truthy = [
            EndpointClass::OpenAiPublic,
            EndpointClass::OpenAiCodex,
            EndpointClass::AzureOpenAi,
            EndpointClass::OpenRouter,
            EndpointClass::GroqNative,
            EndpointClass::CerebrasNative,
            EndpointClass::XAiNative,
        ];
        for class in truthy {
            assert!(
                resolve_capabilities(class).supports_logprobs,
                "{:?} should support logprobs",
                class
            );
        }
        let falsy = [
            EndpointClass::AnthropicPublic,
            EndpointClass::DeepSeekNative,
            EndpointClass::MistralPublic,
            EndpointClass::MoonshotNative,
            EndpointClass::Local,
            EndpointClass::Custom,
        ];
        for class in falsy {
            assert!(
                !resolve_capabilities(class).supports_logprobs,
                "{:?} should NOT support logprobs",
                class
            );
        }
    }

    #[test]
    fn apply_strips_logprobs_when_unsupported() {
        let policy = build_payload_policy(Some("https://api.deepseek.com"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert("logprobs".into(), serde_json::json!(true));
        payload.insert("top_logprobs".into(), serde_json::json!(5));
        payload.insert("model".into(), serde_json::Value::String("dsk".into()));

        policy.apply(&mut payload);

        assert!(payload.get("logprobs").is_none());
        assert!(payload.get("top_logprobs").is_none());
        assert!(payload.get("model").is_some());
    }

    #[test]
    fn apply_keeps_logprobs_when_supported() {
        let policy = build_payload_policy(Some("https://api.groq.com"), "openai-chat", None);
        let mut payload = serde_json::Map::new();
        payload.insert("logprobs".into(), serde_json::json!(true));
        payload.insert("top_logprobs".into(), serde_json::json!(3));

        policy.apply(&mut payload);

        assert_eq!(payload.get("logprobs"), Some(&serde_json::json!(true)));
        assert_eq!(payload.get("top_logprobs"), Some(&serde_json::json!(3)));
    }

    #[test]
    fn cycle3_flipped_endpoints_support_response_format() {
        for class in [
            EndpointClass::AzureOpenAi,
            EndpointClass::OpenRouter,
            EndpointClass::DeepSeekNative,
            EndpointClass::GroqNative,
            EndpointClass::MistralPublic,
            EndpointClass::MoonshotNative,
            EndpointClass::CerebrasNative,
            EndpointClass::XAiNative,
        ] {
            assert!(
                resolve_capabilities(class).supports_response_format,
                "{:?} should support response_format after Cycle 3 flip",
                class
            );
        }
    }

    #[test]
    fn anthropic_local_custom_still_skip_response_format() {
        for class in [
            EndpointClass::AnthropicPublic,
            EndpointClass::Local,
            EndpointClass::Custom,
        ] {
            assert!(
                !resolve_capabilities(class).supports_response_format,
                "{:?} should NOT support response_format",
                class
            );
        }
    }

    #[test]
    fn kimi_policy_derefs_refs_and_fills_types() {
        let policy =
            build_payload_policy(Some("https://api.kimi.com/coding/v1"), "openai-chat", None);
        assert_eq!(policy.endpoint_class, EndpointClass::MoonshotNative);

        let mut schema = serde_json::json!({
            "$defs": {
                "Action": {
                    "oneOf": [
                        { "type": "string", "const": "start" },
                        { "type": "string", "const": "stop" }
                    ]
                }
            },
            "type": "object",
            "properties": {
                "action": { "$ref": "#/$defs/Action" },
                "count": { "enum": [1, 2, 3] }
            }
        });
        policy.apply_to_schema(&mut schema);

        assert!(schema.get("$defs").is_none());
        assert!(schema["properties"]["action"].get("$ref").is_none());
        assert!(schema["properties"]["action"]["oneOf"].is_array());
        // Missing-type enum property gets filled in.
        assert_eq!(schema["properties"]["count"]["type"], "integer");
    }
}
