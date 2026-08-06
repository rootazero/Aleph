use super::*;

#[test]
fn test_presets_contain_known_vendors() {
    // OpenAI-compatible (original)
    assert!(PRESETS.contains_key("deepseek"));
    assert!(PRESETS.contains_key("moonshot"));
    assert!(PRESETS.contains_key("doubao"));
    assert!(PRESETS.contains_key("siliconflow"));
    assert!(PRESETS.contains_key("zhipu"));
    assert!(PRESETS.contains_key("glm"));
    assert!(PRESETS.contains_key("minimax"));
    assert!(PRESETS.contains_key("openai"));

    // Native protocols
    assert!(PRESETS.contains_key("claude"));
    assert!(PRESETS.contains_key("gemini"));

    // Tier 1: High-priority OpenAI-compatible
    assert!(PRESETS.contains_key("groq"));
    assert!(PRESETS.contains_key("together"));
    assert!(PRESETS.contains_key("perplexity"));
    assert!(PRESETS.contains_key("mistral"));

    // Tier 2: Medium-priority OpenAI-compatible
    assert!(PRESETS.contains_key("cohere"));
    assert!(PRESETS.contains_key("fireworks"));
    assert!(PRESETS.contains_key("anyscale"));
    assert!(PRESETS.contains_key("replicate"));

    // Tier 3: Specialized/Regional OpenAI-compatible
    assert!(PRESETS.contains_key("openrouter"));
    assert!(PRESETS.contains_key("lepton"));
    assert!(PRESETS.contains_key("hyperbolic"));

    // Phase B (openclaw parity)
    assert!(PRESETS.contains_key("cerebras"));
    assert!(PRESETS.contains_key("stepfun"));
    assert!(PRESETS.contains_key("huggingface"));
    assert!(PRESETS.contains_key("vertex-anthropic"));
    assert!(PRESETS.contains_key("azure-openai"));
    assert!(PRESETS.contains_key("xai"));
    assert!(PRESETS.contains_key("grok"));
    assert!(PRESETS.contains_key("qwen"));
    assert!(PRESETS.contains_key("dashscope"));
    assert!(PRESETS.contains_key("baichuan"));
    assert!(PRESETS.contains_key("hunyuan"));
    assert!(PRESETS.contains_key("spark"));
}

#[test]
fn test_phase_b_aliases_share_target() {
    // xai / grok point to the same endpoint
    let xai = get_preset("xai").unwrap();
    let grok = get_preset("grok").unwrap();
    assert_eq!(xai.base_url, grok.base_url);

    // qwen / dashscope point to the same endpoint
    let qwen = get_preset("qwen").unwrap();
    let dashscope = get_preset("dashscope").unwrap();
    assert_eq!(qwen.base_url, dashscope.base_url);
}

#[test]
fn test_vertex_anthropic_uses_anthropic_protocol() {
    let p = get_preset("vertex-anthropic").unwrap();
    assert_eq!(p.protocol, "anthropic");
    assert!(p.base_url.contains("aiplatform.googleapis.com"));
}

#[test]
fn test_presets_have_valid_protocol() {
    let valid_protocols = ["openai", "openai-responses", "anthropic", "gemini", "codex"];
    for (name, preset) in PRESETS.iter() {
        assert!(
            valid_protocols.contains(&preset.protocol),
            "Preset '{}' uses invalid protocol '{}'",
            name,
            preset.protocol
        );
    }
}

#[test]
fn test_get_preset_case_insensitive() {
    assert!(get_preset("DeepSeek").is_some());
    assert!(get_preset("MOONSHOT").is_some());
    assert!(get_preset("doubao").is_some());
}

#[test]
fn test_kimi_alias() {
    let moonshot = get_preset("moonshot").unwrap();
    let kimi = get_preset("kimi").unwrap();
    assert_eq!(moonshot.base_url, kimi.base_url);
    // Default `moonshot`/`kimi` preset now ships the recommended anthropic
    // endpoint; the OpenAI endpoint lives at `moonshot-openai`/`kimi-openai`.
    assert_eq!(moonshot.protocol, "anthropic");
}

#[test]
fn test_kimi_for_coding_preset_distinct_from_moonshot() {
    let coding = get_preset("kimi-for-coding").unwrap();
    let coding_alias = get_preset("kimi-coding").unwrap();
    let moonshot = get_preset("moonshot").unwrap();

    assert_eq!(coding.base_url, "https://api.kimi.com/coding/v1");
    assert_eq!(coding.protocol, "anthropic");
    assert_eq!(coding.base_url, coding_alias.base_url);
    // Distinct preset: kimi-for-coding's dedicated coding endpoint vs the
    // general moonshot endpoint. (Both now speak the anthropic protocol, so
    // the distinguisher is the base_url, not the protocol.)
    assert_ne!(coding.base_url, moonshot.base_url);
}

#[test]
fn test_technical_aliases_removed() {
    // These should NOT exist
    assert!(get_preset("anthropic").is_none());
    assert!(get_preset("google").is_none());
}

#[test]
fn test_brand_names_retained() {
    // These should exist
    assert!(get_preset("claude").is_some());
    assert!(get_preset("gemini").is_some());
    assert!(get_preset("kimi").is_some());
    assert!(get_preset("moonshot").is_some());
}

#[test]
fn moonshot_cn_preset_uses_cn_endpoint() {
    let p = get_preset("moonshot-cn").expect("moonshot-cn preset");
    assert_eq!(p.base_url, "https://api.moonshot.cn/v1");
    assert_eq!(p.protocol, "openai");
    assert_eq!(p.default_model, "kimi-k3");
}

#[test]
fn kimi_cn_alias_resolves_to_moonshot_cn() {
    let cn = get_preset("kimi-cn").expect("kimi-cn alias resolves");
    let moonshot_cn = get_preset("moonshot-cn").expect("moonshot-cn preset");
    assert_eq!(cn.base_url, moonshot_cn.base_url);
    assert!(cn.base_url.contains("api.moonshot.cn"));
    let moonshot = get_preset("moonshot").expect("moonshot preset");
    assert_ne!(cn.base_url, moonshot.base_url);
}

#[test]
fn moonshot_fallbacks_include_current_lineup() {
    let p = get_preset("moonshot").expect("moonshot preset");
    // Lineup refreshed to K3 (Moonshot's own catalog default) with the K2.7
    // coding tier and K2.6 below it; K2.5 is retired. The moonshot-v1-* legacy
    // tail is retained.
    assert_eq!(p.default_model, "kimi-k3");
    assert!(p.fallback_models.contains(&"kimi-k3"));
    assert!(p.fallback_models.contains(&"kimi-k2.7-code"));
    assert!(p.fallback_models.contains(&"kimi-k2.6"));
    assert!(!p.fallback_models.contains(&"kimi-k2.5"));
    assert!(p.fallback_models.contains(&"moonshot-v1-8k"));
    // K3 is offered but not defaulted to: ~3.5x the K2.6 rate. The chain is
    // also the picker roster, so this is what makes K3 selectable at all.
    assert!(p.fallback_models.contains(&"kimi-k3"));
    assert!(p.fallback_models.contains(&"kimi-k2.7-code"));
}

/// The Kimi Code endpoint has its own model-id namespace; the roster must
/// carry all four ids the vendor documents, spelled the way that endpoint
/// spells them (`k3`, not `kimi-k3`).
#[test]
fn kimi_for_coding_advertises_the_documented_lineup() {
    let p = get_preset("kimi-for-coding").expect("kimi-for-coding preset");
    assert_eq!(p.default_model, "k3");
    for id in [
        "k3",
        "k3-256k",
        "kimi-for-coding",
        "kimi-for-coding-highspeed",
    ] {
        assert!(
            p.fallback_models.contains(&id),
            "{id} is documented on the Kimi Code endpoint but absent from the roster"
        );
    }
    // Open-platform ids must NOT leak into this preset — the coding endpoint
    // 400s on them (the wire-level translation is a separate safety net).
    assert!(!p.fallback_models.contains(&"kimi-k3"));
}

// =========================================================================
// get_merged_preset tests
// =========================================================================

#[test]
fn test_openrouter_preset_uses_responses() {
    let preset = get_preset("openrouter");
    assert!(preset.is_some());
    let p = preset.unwrap();
    assert_eq!(p.protocol, "openai-responses");
    assert_eq!(p.base_url, "https://openrouter.ai/api");
}

#[test]
fn test_get_merged_preset_builtin_only() {
    let overrides = crate::config::presets_override::PresetsOverride::default();
    let preset = get_merged_preset("openai", &overrides).unwrap();
    assert_eq!(preset.base_url, "https://api.openai.com/v1");
    assert_eq!(preset.protocol, "openai");
    assert_eq!(preset.color, "#10a37f");
    assert_eq!(preset.default_model, "gpt-5.6");
}

#[test]
fn test_get_merged_preset_with_override() {
    let mut overrides = crate::config::presets_override::PresetsOverride::default();
    overrides.providers.insert(
        "openai".to_string(),
        crate::config::presets_override::PartialProviderPreset {
            base_url: Some("https://custom-openai.example.com/v1".to_string()),
            default_model: Some("gpt-5".to_string()),
            enabled: true,
            ..Default::default()
        },
    );

    let preset = get_merged_preset("openai", &overrides).unwrap();
    assert_eq!(preset.base_url, "https://custom-openai.example.com/v1");
    assert_eq!(preset.default_model, "gpt-5");
    // Non-overridden fields fall back to built-in
    assert_eq!(preset.protocol, "openai");
    assert_eq!(preset.color, "#10a37f");
}

#[test]
fn test_get_merged_preset_disabled() {
    let mut overrides = crate::config::presets_override::PresetsOverride::default();
    overrides.providers.insert(
        "openai".to_string(),
        crate::config::presets_override::PartialProviderPreset {
            enabled: false,
            ..Default::default()
        },
    );

    assert!(get_merged_preset("openai", &overrides).is_none());
}

#[test]
fn test_get_merged_preset_new_provider() {
    let mut overrides = crate::config::presets_override::PresetsOverride::default();
    overrides.providers.insert(
        "my-custom-llm".to_string(),
        crate::config::presets_override::PartialProviderPreset {
            base_url: Some("https://my-llm.example.com/v1".to_string()),
            protocol: Some("openai".to_string()),
            color: Some("#abcdef".to_string()),
            default_model: Some("my-model-v1".to_string()),
            enabled: true,
            ..Default::default()
        },
    );

    let preset = get_merged_preset("my-custom-llm", &overrides).unwrap();
    assert_eq!(preset.base_url, "https://my-llm.example.com/v1");
    assert_eq!(preset.protocol, "openai");
    assert_eq!(preset.color, "#abcdef");
    assert_eq!(preset.default_model, "my-model-v1");
}

#[test]
fn test_get_merged_preset_alias_lookup() {
    let mut overrides = crate::config::presets_override::PresetsOverride::default();
    overrides.providers.insert(
        "my-provider".to_string(),
        crate::config::presets_override::PartialProviderPreset {
            base_url: Some("https://alias-test.example.com/v1".to_string()),
            aliases: vec!["alias-one".to_string(), "alias-two".to_string()],
            enabled: true,
            ..Default::default()
        },
    );

    // Look up by alias — no built-in exists for "alias-one"
    let preset = get_merged_preset("alias-one", &overrides).unwrap();
    assert_eq!(preset.base_url, "https://alias-test.example.com/v1");
}

#[test]
fn test_get_merged_preset_case_insensitive() {
    let overrides = crate::config::presets_override::PresetsOverride::default();
    let preset = get_merged_preset("OpenAI", &overrides).unwrap();
    assert_eq!(preset.base_url, "https://api.openai.com/v1");
}

#[test]
fn test_get_merged_preset_not_found() {
    let overrides = crate::config::presets_override::PresetsOverride::default();
    assert!(get_merged_preset("nonexistent-provider", &overrides).is_none());
}

#[test]
fn test_provider_metadata_lookup() {
    let meta = provider_metadata("DeepSeek").expect("deepseek metadata present");
    assert_eq!(meta.display_name, "DeepSeek");
    assert!(meta.supports(Modality::Chat));
    assert!(!meta.supports(Modality::Image));
    // Missing entries return None.
    assert!(provider_metadata("nonexistent").is_none());
}

#[test]
fn test_presets_by_modality_chat_includes_all() {
    // Every shipped preset is chat-capable today. The listing iterates
    // canonical profiles only — aliases resolve but do not get their own
    // row, so the count is the canonical profile count, not PRESETS' (which
    // is alias-expanded for lookup).
    let chat = presets_by_modality(Modality::Chat);
    assert_eq!(chat.len(), canonical_profiles().len());
    assert!(chat.len() < PRESETS.len());
    assert!(chat.contains(&"openai"));
    assert!(chat.contains(&"claude"));
    assert!(chat.contains(&"gemini"));
    // Should be sorted.
    let mut sorted = chat.clone();
    sorted.sort_unstable();
    assert_eq!(chat, sorted);
}

#[test]
fn test_presets_by_modality_image_currently_empty() {
    // No chat preset declares Image yet — generation lives in the
    // separate generation layer (see Phase A in generation/presets).
    assert!(presets_by_modality(Modality::Image).is_empty());
    assert!(presets_by_modality(Modality::Video).is_empty());
}

// =========================================================================
// hermes-parity field tests
// =========================================================================

#[test]
fn alias_dedup_keeps_canonical_data() {
    // moonshot is canonical; kimi is its alias and must resolve to the
    // same data (no duplicate source-of-truth entry).
    let moonshot = get_preset("moonshot").unwrap();
    let kimi = get_preset("kimi").unwrap();
    assert_eq!(moonshot.base_url, kimi.base_url);
    assert_eq!(moonshot.default_model, kimi.default_model);
    assert!(moonshot.aliases.contains(&"kimi"));
}

#[test]
fn high_value_presets_have_signup_url() {
    for name in ["openai", "claude", "deepseek", "moonshot", "gemini"] {
        let p = get_preset(name).unwrap();
        assert!(
            p.signup_url.is_some(),
            "{name} should declare a signup_url for the picker"
        );
    }
}

#[test]
fn high_value_presets_have_fallback_models() {
    for name in [
        "openai", "claude", "deepseek", "moonshot", "gemini", "qwen", "xai",
    ] {
        let p = get_preset(name).unwrap();
        assert!(
            !p.fallback_models.is_empty(),
            "{name} should declare fallback_models for graceful discovery degradation"
        );
    }
}

#[test]
fn alias_resolves_to_canonical_profile() {
    // `codex` is an alias of the `chatgpt` preset — the alias mechanism, not
    // a hardcoded special case in the gateway handlers.
    let chatgpt = get_preset("chatgpt").unwrap();
    let codex = get_preset("codex").unwrap();
    assert_eq!(codex.base_url, chatgpt.base_url);
    assert_eq!(codex.protocol, chatgpt.protocol);
    assert_eq!(canonical_preset_id("codex"), Some("chatgpt"));
    assert_eq!(canonical_preset_id("ChatGPT"), Some("chatgpt"));
    assert_eq!(canonical_preset_id("kimi"), Some("moonshot"));
    assert_eq!(canonical_preset_id("not-a-provider"), None);
}

#[test]
fn modality_listing_has_no_alias_duplicates() {
    // Aliases are resolution keys, not display rows: the enumeration behind
    // the picker and `list_models` must contain each provider exactly once.
    let chat = presets_by_modality(crate::providers::metadata::Modality::Chat);
    let mut seen = std::collections::HashSet::new();
    for name in &chat {
        assert!(seen.insert(name), "duplicate listing row for {name}");
        assert_eq!(
            canonical_preset_id(name),
            Some(*name),
            "{name} is an alias, not a canonical listing row"
        );
    }
    assert!(chat.contains(&"moonshot"));
    assert!(!chat.contains(&"kimi"));
    assert!(!chat.contains(&"codex"));
}

#[test]
fn resolve_models_url_uses_override_then_base_url() {
    // Claude declares a custom models endpoint.
    let claude = get_preset("claude").unwrap();
    assert_eq!(
        claude.resolve_models_url(),
        "https://api.anthropic.com/v1/models"
    );

    // OpenAI uses the {base_url}/models default.
    let openai = get_preset("openai").unwrap();
    assert_eq!(
        openai.resolve_models_url(),
        "https://api.openai.com/v1/models"
    );
}

#[test]
fn kimi_for_coding_omits_temperature() {
    // Kimi-for-coding server-manages temperature → policy = Omit so the
    // chat builder strips it from outgoing requests.
    let p = get_preset("kimi-for-coding").unwrap();
    assert_eq!(p.temperature_policy, Some(super::TemperaturePolicy::Omit));
}

#[test]
fn new_hermes_parity_presets_are_present() {
    // Pulled in from hermes-agent plugins/model-providers.
    for name in [
        "ai-gateway",
        "azure-foundry",
        "gmi",
        "nous",
        "zai",
        "ollama-cloud",
    ] {
        assert!(
            PRESETS.contains_key(name),
            "{name} should be in the registry after the hermes-parity import"
        );
    }
}

#[test]
fn no_health_check_propagated_for_resource_placeholders() {
    // Presets that ship with `YOUR-RESOURCE` / `ACCOUNT_ID` placeholders or
    // OAuth-only endpoints must opt out of `/models` health probing.
    for name in [
        "chatgpt",
        "azure-openai",
        "azure-foundry",
        "amazon-bedrock",
        "vertex-anthropic",
        "ai-gateway",
    ] {
        let p = get_preset(name).unwrap();
        assert!(
            !p.supports_health_check,
            "{name} should set supports_health_check = false"
        );
    }
}

#[test]
fn aliases_resolve_via_get_preset_for_all_groups() {
    // Every alias declared on PROFILES must be reachable through PRESETS.
    let alias_groups = [
        ("moonshot", &["kimi"][..]),
        ("kimi-for-coding", &["kimi-coding"]),
        ("doubao", &["volcengine", "ark"]),
        ("zhipu", &["glm"]),
        ("xai", &["grok"]),
        ("qwen", &["dashscope"]),
        ("amazon-bedrock", &["bedrock"]),
        ("cloudflare-ai", &["workers-ai"]),
        ("github-copilot", &["copilot"]),
        ("nvidia-nim", &["nvidia"]),
        ("openrouter", &["or"]),
    ];

    for (canonical, aliases) in alias_groups {
        let c = get_preset(canonical).expect(canonical);
        for a in aliases {
            let entry = get_preset(a).unwrap_or_else(|| panic!("{a} missing"));
            assert_eq!(
                entry.base_url, c.base_url,
                "alias {a} should mirror {canonical}"
            );
        }
    }
}

// =========================================================================
// temperature_policy wiring (Phase 2.2)
// =========================================================================

#[test]
fn apply_temperature_policy_pass_through_when_none() {
    assert_eq!(apply_temperature_policy(None, Some(0.7)), Some(0.7));
    assert_eq!(apply_temperature_policy(None, None), None);
}

#[test]
fn apply_temperature_policy_omit_drops_value() {
    assert_eq!(
        apply_temperature_policy(Some(super::TemperaturePolicy::Omit), Some(0.7)),
        None
    );
    assert_eq!(
        apply_temperature_policy(Some(super::TemperaturePolicy::Omit), None),
        None
    );
}

#[test]
fn apply_temperature_policy_force_overrides_caller() {
    assert_eq!(
        apply_temperature_policy(Some(super::TemperaturePolicy::Force(1.0)), Some(0.2)),
        Some(1.0)
    );
    assert_eq!(
        apply_temperature_policy(Some(super::TemperaturePolicy::Force(1.0)), None),
        Some(1.0)
    );
}

#[test]
fn temperature_for_base_url_omits_for_kimi_for_coding() {
    // Kimi-for-coding server manages temperature; even when the user sets
    // one, the policy must strip it from the outgoing request.
    let kimi = get_preset("kimi-for-coding").unwrap();
    let resolved = temperature_for_base_url(Some(kimi.base_url), Some(0.7));
    assert_eq!(resolved, None, "kimi-for-coding must omit temperature");
}

#[test]
fn temperature_for_base_url_passes_through_for_normal_presets() {
    // Standard providers (no policy) pass the caller's value through.
    let openai = get_preset("openai").unwrap();
    let resolved = temperature_for_base_url(Some(openai.base_url), Some(0.5));
    assert_eq!(resolved, Some(0.5));
}

#[test]
fn temperature_for_base_url_unknown_url_is_pass_through() {
    // User has overridden base_url to something off-registry — preset
    // policy doesn't apply; caller's value flows through.
    let resolved = temperature_for_base_url(Some("https://my.custom.proxy/v1"), Some(0.9));
    assert_eq!(resolved, Some(0.9));
}

#[test]
fn temperature_for_base_url_none_base_url_is_pass_through() {
    assert_eq!(temperature_for_base_url(None, Some(0.42)), Some(0.42));
    assert_eq!(temperature_for_base_url(None, None), None);
}

#[test]
fn test_get_merged_preset_new_provider_no_base_url() {
    let mut overrides = crate::config::presets_override::PresetsOverride::default();
    overrides.providers.insert(
        "incomplete-provider".to_string(),
        crate::config::presets_override::PartialProviderPreset {
            // Missing base_url — partial_to_provider_preset returns None
            protocol: Some("openai".to_string()),
            enabled: true,
            ..Default::default()
        },
    );

    assert!(get_merged_preset("incomplete-provider", &overrides).is_none());
}

#[test]
fn minimax_primary_is_anthropic_endpoint() {
    let p = PRESETS.get("minimax").expect("minimax preset");
    assert_eq!(p.protocol, "anthropic");
    assert!(
        p.base_url.contains("minimaxi.com/anthropic"),
        "{}",
        p.base_url
    );
    let openai = PRESETS
        .get("minimax-openai")
        .expect("minimax-openai secondary");
    assert_eq!(openai.protocol, "openai");
}

#[test]
fn moonshot_primary_is_anthropic_endpoint() {
    let p = PRESETS.get("moonshot").expect("moonshot preset");
    assert_eq!(p.protocol, "anthropic");
    assert!(
        p.base_url.contains("moonshot.cn/anthropic"),
        "{}",
        p.base_url
    );
    let openai = PRESETS
        .get("moonshot-openai")
        .expect("moonshot-openai secondary");
    assert_eq!(openai.protocol, "openai");
}

#[test]
fn kimi_minimax_govern_as_strict_regardless_of_protocol() {
    // Regression guard: switching the default to the anthropic protocol must
    // NOT loosen governance — vendor_identity still resolves "strict".
    use crate::providers::model_behaviors::vendor_identity;
    assert_eq!(
        vendor_identity(
            Some("https://api.moonshot.cn/anthropic"),
            "kimi-k2-0905-preview"
        ),
        Some("strict")
    );
    assert_eq!(
        vendor_identity(Some("https://api.minimaxi.com/anthropic"), "MiniMax-M2.5"),
        Some("strict")
    );
}
