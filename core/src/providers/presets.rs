//! Provider presets registry
//!
//! Contains default configurations for known AI providers.

use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Provider preset configuration
#[derive(Debug, Clone)]
pub struct ProviderPreset {
    /// Default base URL for the provider
    pub base_url: &'static str,
    /// Protocol to use (e.g., "openai", "anthropic")
    pub protocol: &'static str,
    /// Default color for UI
    pub color: &'static str,
    /// Default model for the provider
    pub default_model: &'static str,
}

/// Registry of known provider presets
pub static PRESETS: Lazy<HashMap<&'static str, ProviderPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // OpenAI official
    m.insert(
        "openai",
        ProviderPreset {
            base_url: "https://api.openai.com/v1",
            protocol: "openai",
            color: "#10a37f",
            default_model: "gpt-4o",
        },
    );

    // ChatGPT subscription (via Codex Responses API, OAuth login)
    m.insert(
        "chatgpt",
        ProviderPreset {
            base_url: "https://chatgpt.com",
            protocol: "chatgpt",
            color: "#10a37f",
            default_model: "gpt-5.3-codex",
        },
    );

    // DeepSeek
    m.insert(
        "deepseek",
        ProviderPreset {
            base_url: "https://api.deepseek.com",
            protocol: "openai",
            color: "#0066cc",
            default_model: "deepseek-chat",
        },
    );

    // Moonshot / Kimi
    m.insert(
        "moonshot",
        ProviderPreset {
            base_url: "https://api.moonshot.cn/v1",
            protocol: "openai",
            color: "#6366f1",
            default_model: "moonshot-v1-8k",
        },
    );
    m.insert(
        "kimi",
        ProviderPreset {
            base_url: "https://api.moonshot.cn/v1",
            protocol: "openai",
            color: "#6366f1",
            default_model: "moonshot-v1-8k",
        },
    );

    // Volcengine Doubao
    m.insert(
        "doubao",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "doubao-1.5-pro-256k",
        },
    );
    m.insert(
        "volcengine",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "doubao-1.5-pro-256k",
        },
    );
    m.insert(
        "ark",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "doubao-1.5-pro-256k",
        },
    );

    // SiliconFlow — Chinese AI cloud platform
    m.insert(
        "siliconflow",
        ProviderPreset {
            base_url: "https://api.siliconflow.cn/v1",
            protocol: "openai",
            color: "#6c5ce7",
            default_model: "deepseek-ai/DeepSeek-V3",
        },
    );

    // Zhipu GLM — Chinese AI research lab
    m.insert(
        "zhipu",
        ProviderPreset {
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            protocol: "openai",
            color: "#3b5998",
            default_model: "GLM-5",
        },
    );
    m.insert(
        "glm",
        ProviderPreset {
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            protocol: "openai",
            color: "#3b5998",
            default_model: "GLM-5",
        },
    );

    // MiniMax — Chinese multimodal AI
    m.insert(
        "minimax",
        ProviderPreset {
            base_url: "https://api.minimax.io/v1",
            protocol: "openai",
            color: "#e84393",
            default_model: "MiniMax-M2.5",
        },
    );

    // T8Star
    m.insert(
        "t8star",
        ProviderPreset {
            base_url: "https://api.t8star.cn/v1",
            protocol: "openai",
            color: "#f59e0b",
            default_model: "",
        },
    );

    // Anthropic Claude
    m.insert(
        "claude",
        ProviderPreset {
            base_url: "https://api.anthropic.com",
            protocol: "anthropic",
            color: "#d97757",
            default_model: "claude-sonnet-4-5-20250514",
        },
    );

    // Google Gemini
    m.insert(
        "gemini",
        ProviderPreset {
            base_url: "https://generativelanguage.googleapis.com",
            protocol: "gemini",
            color: "#4285f4",
            default_model: "gemini-2.5-flash",
        },
    );

    // Groq - Ultra-fast inference
    m.insert(
        "groq",
        ProviderPreset {
            base_url: "https://api.groq.com/openai/v1",
            protocol: "openai",
            color: "#f55036",
            default_model: "llama-3.3-70b-versatile",
        },
    );

    // Together.ai - Open source models
    m.insert(
        "together",
        ProviderPreset {
            base_url: "https://api.together.xyz/v1",
            protocol: "openai",
            color: "#6366f1",
            default_model: "",
        },
    );

    // Perplexity - Search-augmented LLMs
    m.insert(
        "perplexity",
        ProviderPreset {
            base_url: "https://api.perplexity.ai",
            protocol: "openai",
            color: "#20808d",
            default_model: "",
        },
    );

    // Mistral AI - European AI leader
    m.insert(
        "mistral",
        ProviderPreset {
            base_url: "https://api.mistral.ai/v1",
            protocol: "openai",
            color: "#ff7000",
            default_model: "",
        },
    );

    // Cohere - Enterprise focus
    m.insert(
        "cohere",
        ProviderPreset {
            base_url: "https://api.cohere.ai/v1",
            protocol: "openai",
            color: "#39594d",
            default_model: "",
        },
    );

    // Fireworks.ai - Fast API
    m.insert(
        "fireworks",
        ProviderPreset {
            base_url: "https://api.fireworks.ai/inference/v1",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "",
        },
    );

    // Anyscale - Ray ecosystem
    m.insert(
        "anyscale",
        ProviderPreset {
            base_url: "https://api.endpoints.anyscale.com/v1",
            protocol: "openai",
            color: "#00d4aa",
            default_model: "",
        },
    );

    // Replicate - OSS model hosting
    m.insert(
        "replicate",
        ProviderPreset {
            base_url: "https://api.replicate.com/v1",
            protocol: "openai",
            color: "#0c0c0d",
            default_model: "",
        },
    );

    // OpenRouter - Multi-model router
    m.insert(
        "openrouter",
        ProviderPreset {
            base_url: "https://openrouter.ai/api/v1",
            protocol: "openai",
            color: "#7c3aed",
            default_model: "anthropic/claude-sonnet-4-5",
        },
    );

    // Lepton AI - Model deployment
    m.insert(
        "lepton",
        ProviderPreset {
            base_url: "https://api.lepton.ai/api/v1",
            protocol: "openai",
            color: "#4f46e5",
            default_model: "",
        },
    );

    // Hyperbolic - GPU marketplace
    m.insert(
        "hyperbolic",
        ProviderPreset {
            base_url: "https://api.hyperbolic.xyz/v1",
            protocol: "openai",
            color: "#8b5cf6",
            default_model: "",
        },
    );

    m
});

/// Get a preset by name (case-insensitive)
pub fn get_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.get(name.to_lowercase().as_str())
}

/// Get a preset with override support.
///
/// Resolution order:
/// 1. If a user override exists for the name (or via alias), merge it onto the built-in preset.
/// 2. If only a built-in preset exists, convert it to owned form.
/// 3. If only a user override exists (new provider), create from partial.
/// 4. Returns `None` if disabled or not found.
pub fn get_merged_preset(
    name: &str,
    overrides: &crate::config::presets_override::PresetsOverride,
) -> Option<crate::config::presets_override::OwnedProviderPreset> {
    let lower = name.to_lowercase();
    let builtin = PRESETS.get(lower.as_str());
    let partial = overrides
        .providers
        .get(&lower)
        .or_else(|| {
            // Check aliases in user overrides
            overrides.providers.values().find(|p| {
                p.aliases.iter().any(|a| a.to_lowercase() == lower)
            })
        });

    match (builtin, partial) {
        (Some(b), Some(p)) => {
            if !p.enabled {
                return None;
            }
            Some(crate::config::presets_override::merge_provider_preset(b, p))
        }
        (Some(b), None) => Some(crate::config::presets_override::OwnedProviderPreset {
            base_url: b.base_url.to_string(),
            protocol: b.protocol.to_string(),
            color: b.color.to_string(),
            default_model: b.default_model.to_string(),
        }),
        (None, Some(p)) => {
            if !p.enabled {
                return None;
            }
            crate::config::presets_override::partial_to_provider_preset(p)
        }
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn test_presets_have_valid_protocol() {
        let valid_protocols = ["openai", "anthropic", "gemini", "chatgpt"];
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

    // =========================================================================
    // get_merged_preset tests
    // =========================================================================

    #[test]
    fn test_get_merged_preset_builtin_only() {
        let overrides = crate::config::presets_override::PresetsOverride::default();
        let preset = get_merged_preset("openai", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://api.openai.com/v1");
        assert_eq!(preset.protocol, "openai");
        assert_eq!(preset.color, "#10a37f");
        assert_eq!(preset.default_model, "gpt-4o");
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
}
