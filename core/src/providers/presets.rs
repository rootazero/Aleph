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
    m.insert(
        "anthropic",
        ProviderPreset {
            base_url: "https://api.anthropic.com",
            protocol: "anthropic",
            color: "#d97757",
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
        assert!(PRESETS.contains_key("deepseek"));
        assert!(PRESETS.contains_key("moonshot"));
        assert!(PRESETS.contains_key("doubao"));
        assert!(PRESETS.contains_key("openai"));
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
}
