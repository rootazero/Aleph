//! Provider-specific payload policy for OpenAI-compatible protocols.
//!
//! Detects endpoint provider class from base URL, then applies per-provider
//! field filtering/injection for OpenAI Chat and Responses APIs.

use std::collections::HashSet;

// =============================================================================
// EndpointClass
// =============================================================================

/// Detected provider endpoint class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointClass {
    /// Official OpenAI API (api.openai.com)
    OpenAiPublic,
    /// OpenAI Codex (chatgpt.com)
    OpenAiCodex,
    /// Azure OpenAI Service (*.openai.azure.com)
    AzureOpenAi,
    /// Anthropic public API (api.anthropic.com)
    AnthropicPublic,
    /// DeepSeek native API (api.deepseek.com)
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
    /// OpenRouter (openrouter.ai)
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
    /// Supports server-side context compaction
    pub supports_server_compaction: bool,
    /// Known to reject object schemas without `properties`
    pub requires_object_properties: bool,
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
    }
}

// =============================================================================
// Endpoint Detection
// =============================================================================

/// Detect endpoint class from base URL.
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
        "api.moonshot.ai" | "api.moonshot.cn" => EndpointClass::MoonshotNative,
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
        format!("https://{}", url)
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

    local_hosts.contains(host)
        || host.ends_with(".localhost")
        || host.ends_with(".local")
}

// =============================================================================
// Capability Resolution
// =============================================================================

/// Resolve capabilities for a given endpoint class.
pub fn resolve_capabilities(class: EndpointClass) -> ProviderCapabilities {
    match class {
        EndpointClass::OpenAiPublic => ProviderCapabilities {
            supports_responses_store: true,
            supports_reasoning_effort: true,
            supports_prompt_cache: true,
            supports_service_tier: true,
            supports_strict_schema: true,
            supports_response_format: true,
            supports_server_compaction: true,
            requires_object_properties: false,
            context_window: Some(128_000),
        },
        EndpointClass::OpenAiCodex => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: true,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: true,
            supports_response_format: true,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(128_000),
        },
        EndpointClass::AzureOpenAi => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: true,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: None,
        },
        EndpointClass::AnthropicPublic => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: true,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(200_000),
        },
        EndpointClass::DeepSeekNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(64_000),
        },
        EndpointClass::GroqNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(8_000),
        },
        EndpointClass::MistralPublic => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: true,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(128_000),
        },
        EndpointClass::MoonshotNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(128_000),
        },
        EndpointClass::CerebrasNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(128_000),
        },
        EndpointClass::XAiNative => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: Some(128_000),
        },
        EndpointClass::OpenRouter => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: None,
        },
        EndpointClass::Local => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: None,
        },
        EndpointClass::Custom => ProviderCapabilities {
            supports_responses_store: false,
            supports_reasoning_effort: false,
            supports_prompt_cache: false,
            supports_service_tier: false,
            supports_strict_schema: false,
            supports_response_format: false,
            supports_server_compaction: false,
            requires_object_properties: true,
            context_window: None,
        },
    }
}

// =============================================================================
// Policy Builder
// =============================================================================

/// Build a payload policy from provider configuration.
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
        (None, false)
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
    }

    #[test]
    fn test_build_policy_for_openai() {
        let policy = build_payload_policy(
            Some("https://api.openai.com"),
            "openai-responses",
            None,
        );
        assert_eq!(policy.endpoint_class, EndpointClass::OpenAiPublic);
        assert_eq!(policy.explicit_store, Some(true));
        assert!(!policy.strip_reasoning);
        assert_eq!(policy.compaction_threshold, Some(89_600));
    }

    #[test]
    fn test_build_policy_for_deepseek() {
        let policy = build_payload_policy(
            Some("https://api.deepseek.com"),
            "openai-responses",
            None,
        );
        assert_eq!(policy.endpoint_class, EndpointClass::DeepSeekNative);
        assert!(policy.strip_store);
        assert!(policy.strip_reasoning);
        assert!(policy.compaction_threshold.is_none());
    }

    #[test]
    fn test_apply_policy_strips_fields() {
        let policy = build_payload_policy(
            Some("https://api.deepseek.com"),
            "openai-chat",
            None,
        );
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
        let policy = build_payload_policy(
            Some("https://api.openai.com"),
            "openai-responses",
            None,
        );
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
    fn test_detect_openrouter() {
        assert_eq!(
            detect_endpoint_class(Some("https://openrouter.ai/api/v1")),
            EndpointClass::OpenRouter
        );
    }

    #[test]
    fn test_detect_default_is_openai() {
        assert_eq!(
            detect_endpoint_class(None),
            EndpointClass::OpenAiPublic
        );
        assert_eq!(
            detect_endpoint_class(Some("")),
            EndpointClass::OpenAiPublic
        );
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
    fn third_party_endpoints_do_not_support_response_format() {
        for class in [
            EndpointClass::AzureOpenAi,
            EndpointClass::AnthropicPublic,
            EndpointClass::DeepSeekNative,
            EndpointClass::GroqNative,
            EndpointClass::MistralPublic,
            EndpointClass::MoonshotNative,
            EndpointClass::CerebrasNative,
            EndpointClass::XAiNative,
            EndpointClass::OpenRouter,
            EndpointClass::Local,
            EndpointClass::Custom,
        ] {
            let caps = resolve_capabilities(class);
            assert!(
                !caps.supports_response_format,
                "{:?} unexpectedly supports response_format",
                class
            );
        }
    }

    #[test]
    fn apply_strips_response_format_when_unsupported() {
        let policy = build_payload_policy(
            Some("https://api.deepseek.com"),
            "openai-chat",
            None,
        );
        let mut payload = serde_json::Map::new();
        payload.insert(
            "response_format".into(),
            serde_json::json!({"type": "json_object"}),
        );
        payload.insert("model".into(), serde_json::Value::String("dsk".into()));

        policy.apply(&mut payload);

        assert!(payload.get("response_format").is_none());
        assert!(payload.get("model").is_some());
    }

    #[test]
    fn apply_keeps_response_format_when_supported() {
        let policy = build_payload_policy(
            Some("https://api.openai.com"),
            "openai-chat",
            None,
        );
        let mut payload = serde_json::Map::new();
        payload.insert(
            "response_format".into(),
            serde_json::json!({"type": "json_object"}),
        );

        policy.apply(&mut payload);

        assert!(payload.get("response_format").is_some());
    }
}
