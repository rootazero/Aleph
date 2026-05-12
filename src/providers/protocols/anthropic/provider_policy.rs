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
// AnthropicPolicy
// =============================================================================

/// Resolved Anthropic-protocol policy for a given config.
#[derive(Debug, Clone)]
pub struct AnthropicPolicy {
    pub class: AnthropicEndpointClass,
    pub capabilities: AnthropicCapabilities,
}

impl AnthropicPolicy {
    /// Strip fields from the serialized request body whose capability bit is
    /// `false`. Called once at the end of `build_request`, after
    /// `serde_json::to_value(&request_body)`.
    ///
    /// `cache_control` is NOT stripped here — it's gated at injection time in
    /// `build_request` (see `policy.capabilities.supports_cache_control` check).
    pub fn apply(&self, body: &mut serde_json::Value) {
        let Some(obj) = body.as_object_mut() else { return };
        let caps = &self.capabilities;
        if !caps.supports_service_tier {
            obj.remove("service_tier");
        }
        if !caps.supports_metadata_user_id {
            strip_metadata_user_id(obj);
        }
        if !caps.supports_output_config_effort {
            strip_output_config_effort(obj);
        }
        if !caps.supports_top_k {
            obj.remove("top_k");
        }
        if !caps.supports_top_p {
            obj.remove("top_p");
        }
        if !caps.supports_stop_sequences {
            obj.remove("stop_sequences");
        }
    }
}

/// Remove `metadata.user_id`. If `metadata` becomes empty afterward, remove it too.
fn strip_metadata_user_id(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(metadata) = obj.get_mut("metadata") else { return };
    let Some(map) = metadata.as_object_mut() else { return };
    map.remove("user_id");
    if map.is_empty() {
        obj.remove("metadata");
    }
}

/// Remove `output_config.effort`. If `output_config` becomes empty afterward, remove it too.
fn strip_output_config_effort(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(output_config) = obj.get_mut("output_config") else { return };
    let Some(map) = output_config.as_object_mut() else { return };
    map.remove("effort");
    if map.is_empty() {
        obj.remove("output_config");
    }
}

// =============================================================================
// Policy Builder
// =============================================================================

/// Build a complete Anthropic policy from configuration.
pub fn build_anthropic_policy(base_url: Option<&str>) -> AnthropicPolicy {
    let class = detect_anthropic_endpoint_class(base_url);
    let capabilities = resolve_anthropic_capabilities(class);
    AnthropicPolicy { class, capabilities }
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

    fn body_with_all_anthropic_fields() -> serde_json::Value {
        serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [],
            "max_tokens": 1024,
            "temperature": 0.7,
            "top_p": 0.9,
            "top_k": 40,
            "stop_sequences": ["END", "STOP"],
            "service_tier": "auto",
            "metadata": {"user_id": "u_42"},
            "output_config": {"effort": "high"}
        })
    }

    #[test]
    fn apply_on_custom_strips_service_tier() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert!(body.get("service_tier").is_none());
    }

    #[test]
    fn apply_on_custom_strips_metadata_object_when_user_id_only_field() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert!(body.get("metadata").is_none(),
            "metadata object should be removed when only field (user_id) is stripped");
    }

    #[test]
    fn apply_on_custom_strips_output_config_when_effort_only_field() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn apply_on_custom_keeps_top_k_top_p_stop_sequences() {
        let policy = build_anthropic_policy(Some("https://kimi-for-coding.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["stop_sequences"], serde_json::json!(["END", "STOP"]));
    }

    #[test]
    fn apply_on_official_keeps_all_fields() {
        let policy = build_anthropic_policy(Some("https://api.anthropic.com/v1/messages"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert_eq!(body["service_tier"], "auto");
        assert_eq!(body["metadata"]["user_id"], "u_42");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn apply_on_custom_never_touches_unrelated_fields() {
        let policy = build_anthropic_policy(Some("https://custom.example.com"));
        let mut body = body_with_all_anthropic_fields();
        policy.apply(&mut body);
        assert_eq!(body["model"], "claude-3-5-sonnet");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["temperature"], 0.7);
        assert!(body.get("messages").is_some());
    }

    #[test]
    fn apply_metadata_with_other_keys_keeps_metadata_object() {
        let policy = build_anthropic_policy(Some("https://custom.example.com"));
        let mut body = serde_json::json!({
            "metadata": {"user_id": "u_42", "future_field": "preserved"}
        });
        policy.apply(&mut body);
        // user_id removed, but future_field keeps the object alive
        assert!(body.get("metadata").is_some());
        assert!(body["metadata"].get("user_id").is_none());
        assert_eq!(body["metadata"]["future_field"], "preserved");
    }

    #[test]
    fn apply_on_non_object_body_is_no_op() {
        let policy = build_anthropic_policy(None);
        let mut body = serde_json::json!("not an object");
        policy.apply(&mut body);
        assert_eq!(body, serde_json::json!("not an object"));
    }
}
