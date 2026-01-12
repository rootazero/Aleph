//! General configuration types
//!
//! Contains core application settings:
//! - GeneralConfig: App-wide settings (default provider, logging, language)
//! - ShortcutsConfig: Keyboard shortcuts configuration
//! - BehaviorConfig: Input/output behavior settings
//! - TriggerConfig: Hotkey trigger configuration

use serde::{Deserialize, Serialize};

// =============================================================================
// GeneralConfig
// =============================================================================

/// General configuration settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneralConfig {
    /// Default provider to use when no routing rule matches
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Log retention in days (1-30, default: 7)
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// Enable performance logging (default: false)
    #[serde(default)]
    pub enable_performance_logging: bool,
    /// Preferred language override (e.g., 'en', 'zh-Hans'). If None, use system language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

pub fn default_log_retention_days() -> u32 {
    7 // Keep logs for 7 days by default
}

// =============================================================================
// ShortcutsConfig
// =============================================================================

/// Shortcuts configuration (Phase 6 - Task 4.2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfig {
    /// Global summon hotkey (e.g., "Command+Grave") - LEGACY, not used in new trigger system
    #[serde(default = "default_summon_hotkey")]
    pub summon: String,
    /// Cancel operation hotkey (optional)
    #[serde(default)]
    pub cancel: Option<String>,
    /// Command completion hotkey (e.g., "Command+Option+/")
    /// Format: "Modifier1+Modifier2+Key" where modifiers are Command, Option, Control, Shift
    #[serde(default = "default_command_prompt_hotkey")]
    pub command_prompt: String,
}

pub fn default_hotkey() -> String {
    "Grave".to_string()
}

pub fn default_summon_hotkey() -> String {
    "Command+Grave".to_string()
}

pub fn default_command_prompt_hotkey() -> String {
    "Command+Option+/".to_string()
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

/// Behavior configuration (Phase 6 - Task 5.1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Input mode: "cut" or "copy"
    #[serde(default = "default_input_mode")]
    pub input_mode: String,
    /// Output mode: "typewriter" or "instant"
    #[serde(default = "default_output_mode")]
    pub output_mode: String,
    /// Typing speed in characters per second (10-200)
    #[serde(default = "default_typing_speed")]
    pub typing_speed: u32,
    /// Enable PII scrubbing (email, phone, SSN, etc.)
    #[serde(default)]
    pub pii_scrubbing_enabled: bool,
    /// Enable multi-turn conversation by default
    /// When true, every conversation supports follow-up questions
    /// When false, use /chat command to start multi-turn mode
    #[serde(default)]
    pub multi_turn_enabled: bool,
    /// Keep multi-turn window visible during AI processing
    /// When true, window stays visible and SubPanel shows processing status
    /// When false, window hides during processing and shows indicator instead
    #[serde(default = "default_keep_window_visible")]
    pub keep_window_visible_during_processing: bool,
}

pub fn default_input_mode() -> String {
    "cut".to_string()
}

pub fn default_output_mode() -> String {
    "typewriter".to_string()
}

pub fn default_typing_speed() -> u32 {
    50 // 50 characters per second
}

pub fn default_keep_window_visible() -> bool {
    true // Window stays visible by default
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            input_mode: default_input_mode(),
            output_mode: default_output_mode(),
            typing_speed: default_typing_speed(),
            pii_scrubbing_enabled: false,
            multi_turn_enabled: false,
            keep_window_visible_during_processing: default_keep_window_visible(),
        }
    }
}

// =============================================================================
// TriggerConfig
// =============================================================================

/// Trigger configuration for hotkey system
///
/// Defines hotkeys for Replace and Append operations:
/// - Replace: AI response replaces original text (default: double-tap left Shift)
/// - Append: AI response appends after original text (default: double-tap right Shift)
///
/// # Example TOML
/// ```toml
/// [trigger]
/// replace_hotkey = "DoubleTap+leftShift"
/// append_hotkey = "DoubleTap+rightShift"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Hotkey for Replace action (AI response replaces original text)
    /// Format: "DoubleTap+{modifierKey}"
    /// Supported modifiers: leftShift, rightShift, leftControl, rightControl,
    ///                     leftOption, rightOption, leftCommand, rightCommand
    #[serde(default = "default_replace_hotkey")]
    pub replace_hotkey: String,

    /// Hotkey for Append action (AI response appends after original text)
    /// Format: "DoubleTap+{modifierKey}"
    #[serde(default = "default_append_hotkey")]
    pub append_hotkey: String,
}

pub fn default_replace_hotkey() -> String {
    "DoubleTap+leftShift".to_string()
}

pub fn default_append_hotkey() -> String {
    "DoubleTap+rightShift".to_string()
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            replace_hotkey: default_replace_hotkey(),
            append_hotkey: default_append_hotkey(),
        }
    }
}
