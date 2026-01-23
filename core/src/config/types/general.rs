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
    /// Preferred language override (e.g., 'en', 'zh-Hans'). If None, use system language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Custom output directory for generated files (images, PDFs, etc.)
    /// If None, uses default: ~/.config/aether/output/
    /// This directory is used as the default destination when AI generates files
    /// without specifying an absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

// =============================================================================
// ShortcutsConfig
// =============================================================================

/// Shortcuts configuration (Phase 6 - Task 4.2)
///
/// Note: The `summon` field is LEGACY and not used in the new trigger system.
/// Use TriggerConfig.replace_hotkey and TriggerConfig.append_hotkey instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfig {
    /// Legacy summon hotkey - NOT USED in new trigger system
    /// Kept for backward compatibility with old config files
    #[serde(default = "default_summon_hotkey")]
    pub summon: String,
    /// Cancel operation hotkey (optional)
    #[serde(default)]
    pub cancel: Option<String>,
    /// Command completion hotkey (e.g., "Command+Option+/")
    /// Format: "Modifier1+Modifier2+Key" where modifiers are Command, Option, Control, Shift
    #[serde(default = "default_command_prompt_hotkey")]
    pub command_prompt: String,
    /// OCR capture hotkey (e.g., "Command+Shift+Control+4")
    /// Format: "Modifier1+Modifier2+...+Key"
    #[serde(default = "default_ocr_capture_hotkey")]
    pub ocr_capture: String,
}

/// Legacy default hotkey - NOT USED in new trigger system
/// Kept for backward compatibility
pub fn default_hotkey() -> String {
    "Grave".to_string() // Legacy value, use TriggerConfig instead
}

/// Legacy default summon hotkey - NOT USED in new trigger system
/// Kept for backward compatibility
pub fn default_summon_hotkey() -> String {
    "Command+Grave".to_string() // Legacy value, use TriggerConfig instead
}

pub fn default_command_prompt_hotkey() -> String {
    "Command+Option+/".to_string()
}

pub fn default_ocr_capture_hotkey() -> String {
    "Command+Option+O".to_string()
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            summon: default_summon_hotkey(),
            cancel: Some("Escape".to_string()),
            command_prompt: default_command_prompt_hotkey(),
            ocr_capture: default_ocr_capture_hotkey(),
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    50 // 50 characters per second
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            output_mode: default_output_mode(),
            typing_speed: default_typing_speed(),
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
