//! General configuration types
//!
//! Contains core application settings:
//! - `GeneralConfig`: App-wide settings (default provider, logging, language)
//! - `BehaviorConfig`: Input/output behavior settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// GeneralConfig
// =============================================================================

/// General configuration settings
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GeneralConfig {
    /// Default provider to use when no routing rule matches
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Preferred language override (e.g., 'en', 'zh-Hans'). If None, use system language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Browser system configuration (profiles, SSRF policy, Playwright CLI).
    #[serde(default)]
    pub browser: crate::browser::profile::BrowserSystemConfig,
    /// Global fallback provider chain.
    /// When the default provider fails with a transient error (rate limit, timeout),
    /// these providers are tried in order. Names must match keys in [providers].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_providers: Vec<String>,
    /// Session store backend: `"file"` (default, see
    /// [`default_session_store_backend`]) or `"sqlite"`. The doc used to name
    /// `"sqlite"` as the default while the function two lines down returned
    /// `"file"`, which made every reasoning about an accidental reset of this
    /// key read as a no-op.
    #[serde(default = "default_session_store_backend")]
    pub session_store_backend: String,
}

fn default_session_store_backend() -> String {
    "file".to_string()
}

// =============================================================================
// BehaviorConfig
// =============================================================================

/// Behavior configuration for output mode
///
/// Active fields:
/// - `output_mode`: "typewriter" (character-by-character) or "instant" (all at once)
///
/// `typing_speed` was retired in the 2026-08-17 wire audit (config-003):
/// parsed, returned by `handle_get`, and bounded (50-400 cps) by `handle_update`,
/// but no production code read it to throttle per-second emission. The actual
/// typewriter path keys only on `output_mode`. Existing `config.toml` keys
/// keep parsing because `Config` does not `deny_unknown_fields`.
///
/// Deprecated fields (kept for backward compatibility, ignored by code):
/// - `input_mode`: Replaced by trigger system
/// - `pii_scrubbing_enabled`: Migrated to search.pii.enabled
/// - `multi_turn_enabled`: No longer used
/// - `keep_window_visible_during_processing`: No longer used
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BehaviorConfig {
    /// Output mode: "typewriter" or "instant"
    #[serde(default = "default_output_mode")]
    pub output_mode: String,
}

pub fn default_output_mode() -> String {
    "typewriter".to_string()
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            output_mode: default_output_mode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_in_general_config() {
        let toml_str = r#"
        [browser.profiles.default]
        browser = "chromium"

        [browser.policy]
        block_private = true
        blocked_domains = ["evil.com"]

        [browser.playwright_mcp]
        headless = true
        "#;

        let config: GeneralConfig = toml::from_str(toml_str).unwrap();
        assert!(config.browser.policy.block_private);
        assert_eq!(config.browser.profiles.len(), 1);
        // Legacy [browser.playwright_mcp] alias still maps to playwright_cli.
        assert!(config.browser.playwright_cli.headless);
    }

    #[test]
    fn test_general_config_default_browser() {
        let toml_str = "";
        let config: GeneralConfig = toml::from_str(toml_str).unwrap();
        // Browser config should use defaults
        assert!(config.browser.profiles.is_empty());
    }
}
