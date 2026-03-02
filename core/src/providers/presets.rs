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
        },
    );

    // DeepSeek
    m.insert(
        "deepseek",
        ProviderPreset {
            base_url: "https://api.deepseek.com",
            protocol: "openai",
            color: "#0066cc",
        },
    );

    // Moonshot / Kimi
    m.insert(
        "moonshot",
        ProviderPreset {
            base_url: "https://api.moonshot.cn/v1",
            protocol: "openai",
            color: "#6366f1",
        },
    );
    m.insert(
        "kimi",
        ProviderPreset {
            base_url: "https://api.moonshot.cn/v1",
            protocol: "openai",
            color: "#6366f1",
        },
    );

    // Volcengine Doubao
    m.insert(
        "doubao",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
        },
    );
    m.insert(
        "volcengine",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
        },
    );
    m.insert(
        "ark",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
        },
    );

    // SiliconFlow — Chinese AI cloud platform
    m.insert(
        "siliconflow",
        ProviderPreset {
            base_url: "https://api.siliconflow.cn/v1",
            protocol: "openai",
            color: "#6c5ce7",
        },
    );

    // Zhipu GLM — Chinese AI research lab
    m.insert(
        "zhipu",
        ProviderPreset {
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            protocol: "openai",
            color: "#3b5998",
        },
    );
    m.insert(
        "glm",
        ProviderPreset {
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            protocol: "openai",
            color: "#3b5998",
        },
    );

    // MiniMax — Chinese multimodal AI
    m.insert(
        "minimax",
        ProviderPreset {
            base_url: "https://api.minimax.io/v1",
            protocol: "openai",
            color: "#e84393",
        },
    );

    // T8Star
    m.insert(
        "t8star",
        ProviderPreset {
            base_url: "https://api.t8star.cn/v1",
            protocol: "openai",
            color: "#f59e0b",
        },
    );

    // Anthropic Claude
    m.insert(
        "claude",
        ProviderPreset {
            base_url: "https://api.anthropic.com",
            protocol: "anthropic",
            color: "#d97757",
        },
    );

    // Google Gemini
    m.insert(
        "gemini",
        ProviderPreset {
            base_url: "https://generativelanguage.googleapis.com",
            protocol: "gemini",
            color: "#4285f4",
        },
    );

    // Groq - Ultra-fast inference
    m.insert(
        "groq",
        ProviderPreset {
            base_url: "https://api.groq.com/openai/v1",
            protocol: "openai",
            color: "#f55036",
        },
    );

    // Together.ai - Open source models
    m.insert(
        "together",
        ProviderPreset {
            base_url: "https://api.together.xyz/v1",
            protocol: "openai",
            color: "#6366f1",
        },
    );

    // Perplexity - Search-augmented LLMs
    m.insert(
        "perplexity",
        ProviderPreset {
            base_url: "https://api.perplexity.ai",
            protocol: "openai",
            color: "#20808d",
        },
    );

    // Mistral AI - European AI leader
    m.insert(
        "mistral",
        ProviderPreset {
            base_url: "https://api.mistral.ai/v1",
            protocol: "openai",
            color: "#ff7000",
        },
    );

    // Cohere - Enterprise focus
    m.insert(
        "cohere",
        ProviderPreset {
            base_url: "https://api.cohere.ai/v1",
            protocol: "openai",
            color: "#39594d",
        },
    );

    // Fireworks.ai - Fast API
    m.insert(
        "fireworks",
        ProviderPreset {
            base_url: "https://api.fireworks.ai/inference/v1",
            protocol: "openai",
            color: "#ff6b35",
        },
    );

    // Anyscale - Ray ecosystem
    m.insert(
        "anyscale",
        ProviderPreset {
            base_url: "https://api.endpoints.anyscale.com/v1",
            protocol: "openai",
            color: "#00d4aa",
        },
    );

    // Replicate - OSS model hosting
    m.insert(
        "replicate",
        ProviderPreset {
            base_url: "https://api.replicate.com/v1",
            protocol: "openai",
            color: "#0c0c0d",
        },
    );

    // OpenRouter - Multi-model router
    m.insert(
        "openrouter",
        ProviderPreset {
            base_url: "https://openrouter.ai/api/v1",
            protocol: "openai",
            color: "#7c3aed",
        },
    );

    // Lepton AI - Model deployment
    m.insert(
        "lepton",
        ProviderPreset {
            base_url: "https://api.lepton.ai/api/v1",
            protocol: "openai",
            color: "#4f46e5",
        },
    );

    // Hyperbolic - GPU marketplace
    m.insert(
        "hyperbolic",
        ProviderPreset {
            base_url: "https://api.hyperbolic.xyz/v1",
            protocol: "openai",
            color: "#8b5cf6",
        },
    );

    m
});

/// Get a preset by name (case-insensitive)
pub fn get_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.get(name.to_lowercase().as_str())
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
        let valid_protocols = ["openai", "anthropic", "gemini"];
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
}
