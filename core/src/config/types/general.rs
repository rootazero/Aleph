//! General configuration types
//!
//! Contains core application settings:
//! - GeneralConfig: App-wide settings (default provider, logging, language)
//! - ShortcutsConfig: Keyboard shortcuts configuration
//! - BehaviorConfig: Input/output behavior settings

use crate::agent_loop::QueueMode;
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
    /// Session queue mode: how incoming messages are handled while agent is busy.
    /// Options: "followup" (default), "steer", "collect"
    #[serde(default)]
    pub queue_mode: QueueMode,
    /// Collection window in milliseconds for Collect queue mode.
    /// Only used when queue_mode = "collect". Default: 3000ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collect_window_ms: Option<u64>,
    /// Browser system configuration (profiles, SSRF policy, Playwright MCP).
    #[serde(default)]
    pub browser: crate::browser::profile::BrowserSystemConfig,
}

// =============================================================================
// ShortcutsConfig
// =============================================================================

/// Shortcuts configuration (Phase 6 - Task 4.2)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShortcutsConfig {
    /// Legacy summon hotkey - kept for backward compatibility with old config files
    #[serde(default = "default_summon_hotkey")]
    pub summon: String,
    /// Cancel operation hotkey (optional)
    #[serde(default)]
    pub cancel: Option<String>,
    /// Command completion hotkey (e.g., "Option+Space")
    /// Format: "Modifier1+Modifier2+Key" where modifiers are Command, Option, Control, Shift
    #[serde(default = "default_command_prompt_hotkey")]
    pub command_prompt: String,
}

/// Legacy default hotkey - kept for backward compatibility
pub fn default_hotkey() -> String {
    "Grave".to_string()
}

/// Legacy default summon hotkey - kept for backward compatibility
pub fn default_summon_hotkey() -> String {
    "Command+Grave".to_string()
}

pub fn default_command_prompt_hotkey() -> String {
    "Option+Space".to_string()
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            summon: default_summon_hotkey(),
            cancel: Some("Escape".to_string()),
            command_prompt: default_command_prompt_hotkey(),
        }
    }
}

// =============================================================================
// BehaviorConfig
// =============================================================================

/// Behavior configuration for output mode and typing speed
///
/// Active fields:
/// - output_mode: "typewriter" (character-by-character) or "instant" (all at once)
/// - typing_speed: Characters per second for typewriter mode (50-400)
///
/// Deprecated fields (kept for backward compatibility, ignored by code):
/// - input_mode: Replaced by trigger system
/// - pii_scrubbing_enabled: Migrated to search.pii.enabled
/// - multi_turn_enabled: No longer used
/// - keep_window_visible_during_processing: No longer used
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BehaviorConfig {
    /// Output mode: "typewriter" or "instant"
    #[serde(default = "default_output_mode")]
    pub output_mode: String,
    /// Typing speed in characters per second (50-400)
    #[serde(default = "default_typing_speed")]
    pub typing_speed: u32,
}

pub fn default_output_mode() -> String {
    "typewriter".to_string()
}

pub fn default_typing_speed() -> u32 {
    200 // 200 characters per second
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            output_mode: default_output_mode(),
            typing_speed: default_typing_speed(),
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
        cdp_port = 18800

        [browser.policy]
        block_private = true
        blocked_domains = ["evil.com"]

        [browser.playwright_mcp]
        enabled = true
        "#;

        let config: GeneralConfig = toml::from_str(toml_str).unwrap();
        assert!(config.browser.policy.block_private);
        assert_eq!(config.browser.profiles.len(), 1);
        assert!(config.browser.playwright_mcp.enabled);
    }

    #[test]
    fn test_general_config_default_browser() {
        let toml_str = "";
        let config: GeneralConfig = toml::from_str(toml_str).unwrap();
        // Browser config should use defaults
        assert!(config.browser.profiles.is_empty());
    }
}

