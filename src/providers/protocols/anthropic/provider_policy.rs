//! Anthropic-protocol provider policy.
//!
//! Mirrors `openai_common::provider_policy` but for the Anthropic Messages
//! API. Endpoint classification → capability bits → JSON body mutation.

// =============================================================================
// AnthropicEndpointClass
// =============================================================================

/// Detected Anthropic-protocol endpoint class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnthropicEndpointClass {
    /// Official Anthropic API: host == api.anthropic.com
    Official,
    /// Everything else: kimi-for-coding, MiniMax-anthropic-mode, Bedrock,
    /// Vertex, OpenRouter-anthropic, etc.
    Custom,
}

// =============================================================================
// Endpoint Detection
// =============================================================================

/// Detect Anthropic-protocol endpoint class from base URL.
///
/// `None` / `Some("")` → `Official` (matches `build_endpoint` default).
/// Unparseable URLs → `Custom` (conservative).
pub fn detect_anthropic_endpoint_class(base_url: Option<&str>) -> AnthropicEndpointClass {
    let url = match base_url {
        None | Some("") => return AnthropicEndpointClass::Official,
        Some(u) => u,
    };
    let host = extract_hostname(url).unwrap_or_default().to_lowercase();
    if host == "api.anthropic.com" {
        AnthropicEndpointClass::Official
    } else {
        AnthropicEndpointClass::Custom
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

// =============================================================================
// AnthropicCapabilities
// =============================================================================

/// Per-class capability flags for Anthropic-protocol endpoints.
///
/// Bits resolve solely from endpoint class (single source of truth).
/// There is no `ProviderConfig` override for any bit.
#[derive(Debug, Clone, Default)]
pub struct AnthropicCapabilities {
    /// `cache_control` breakpoints in `system` / `messages`.
    /// Gates `effective_cache_retention` injection in `build_request`.
    pub supports_cache_control: bool,
    /// `service_tier` field ("auto" / "standard_only").
    pub supports_service_tier: bool,
    /// `metadata.user_id` field.
    pub supports_metadata_user_id: bool,
    /// `output_config.effort` field ("low"/"medium"/"high"/"max").
    pub supports_output_config_effort: bool,
    /// `top_k` sampling field.
    pub supports_top_k: bool,
    /// `top_p` sampling field.
    pub supports_top_p: bool,
    /// `stop_sequences` field.
    pub supports_stop_sequences: bool,
}

// =============================================================================
// Capability Resolution
// =============================================================================

/// Resolve capabilities for a given Anthropic endpoint class.
pub fn resolve_anthropic_capabilities(class: AnthropicEndpointClass) -> AnthropicCapabilities {
    match class {
        AnthropicEndpointClass::Official => AnthropicCapabilities {
            supports_cache_control: true,
            supports_service_tier: true,
            supports_metadata_user_id: true,
            supports_output_config_effort: true,
            supports_top_k: true,
            supports_top_p: true,
            supports_stop_sequences: true,
        },
        AnthropicEndpointClass::Custom => AnthropicCapabilities {
            // Conservative: only the universally-portable Anthropic-protocol bits.
            // Premium/Anthropic-only features default OFF — third-party gateways
            // (kimi-for-coding, bedrock, vertex, etc.) reject them with 400 errors.
            supports_cache_control: false,
            supports_service_tier: false,
            supports_metadata_user_id: false,
            supports_output_config_effort: false,
            supports_top_k: true,
            supports_top_p: true,
            supports_stop_sequences: true,
        },
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_official_when_base_url_is_anthropic_host() {
        assert_eq!(
            detect_anthropic_endpoint_class(Some("https://api.anthropic.com/v1/messages")),
            AnthropicEndpointClass::Official
        );
    }

    #[test]
    fn detect_official_when_base_url_is_none() {
        assert_eq!(
            detect_anthropic_endpoint_class(None),
            AnthropicEndpointClass::Official
        );
    }

    #[test]
    fn detect_official_when_base_url_is_empty_string() {
        assert_eq!(
            detect_anthropic_endpoint_class(Some("")),
            AnthropicEndpointClass::Official
        );
    }

    #[test]
    fn detect_custom_for_third_party_hosts() {
        assert_eq!(
            detect_anthropic_endpoint_class(Some("https://kimi-for-coding.example.com/v1/messages")),
            AnthropicEndpointClass::Custom
        );
        assert_eq!(
            detect_anthropic_endpoint_class(Some("https://api.moonshot.cn/anthropic")),
            AnthropicEndpointClass::Custom
        );
    }

    #[test]
    fn detect_custom_for_unparseable_url() {
        // Garbage that even after `https://` prefix won't parse as a URL
        assert_eq!(
            detect_anthropic_endpoint_class(Some("not a url at all !!!")),
            AnthropicEndpointClass::Custom
        );
    }

    #[test]
    fn official_capabilities_have_all_bits_true() {
        let caps = resolve_anthropic_capabilities(AnthropicEndpointClass::Official);
        assert!(caps.supports_cache_control);
        assert!(caps.supports_service_tier);
        assert!(caps.supports_metadata_user_id);
        assert!(caps.supports_output_config_effort);
        assert!(caps.supports_top_k);
        assert!(caps.supports_top_p);
        assert!(caps.supports_stop_sequences);
    }

    #[test]
    fn custom_capabilities_keep_protocol_standard_bits_only() {
        let caps = resolve_anthropic_capabilities(AnthropicEndpointClass::Custom);
        // Anthropic-only bits OFF
        assert!(!caps.supports_cache_control);
        assert!(!caps.supports_service_tier);
        assert!(!caps.supports_metadata_user_id);
        assert!(!caps.supports_output_config_effort);
        // Protocol-standard bits ON
        assert!(caps.supports_top_k);
        assert!(caps.supports_top_p);
        assert!(caps.supports_stop_sequences);
    }
}
