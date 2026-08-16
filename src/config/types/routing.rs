//! Routing configuration types
//!
//! Contains routing rule configuration:
//! - `RoutingRuleConfig`: AI routing rules with command/keyword types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// RoutingRuleConfig
// =============================================================================

/// Routing rule configuration for TOML parsing
///
/// Aleph supports two types of routing rules:
///
/// ## Command Rules
/// - Pattern starts with `^/` (e.g., `^/draw`, `^/translate`)
/// - First-match-stops: only one command rule matches per request
/// - Requires `provider` field to specify which AI to use
/// - Command prefix is automatically stripped before sending to AI
///
/// ## Keyword Rules
/// - Pattern does not start with `/` (e.g., `translate to English`, `code optimization`)
/// - All-match: multiple keyword rules can match simultaneously
/// - No `provider` field (uses `default_provider`)
/// - Multiple matched prompts are combined with `\n\n`
///
/// # Example TOML
///
/// ```toml
/// # Command rule - specifies provider
/// [[rules]]
/// rule_type = "command"
/// regex = "^/draw\\s+"
/// provider = "gemini"
/// system_prompt = "Draw a picture based on the prompt"
///
/// # Keyword rule - prompt only, no provider
/// [[rules]]
/// rule_type = "keyword"
/// regex = "translate to English"
/// system_prompt = "Translate the target language to English"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutingRuleConfig {
    // ===== Rule Type (refactor-routing-rule-logic) =====
    /// Rule type: "command" or "keyword"
    /// - "command": Starts with /, first-match-stops, requires provider
    /// - "keyword": Non-/ pattern, all-match, prompt only
    ///
    /// Default: auto-detected based on regex pattern
    #[serde(default)]
    pub rule_type: Option<String>,

    /// Whether this is a builtin rule (read-only in Settings UI)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_builtin: bool,

    // ===== Core fields =====
    /// Regex pattern to match against user input
    pub regex: String,

    /// Provider name to use when this rule matches
    /// Required for command rules, ignored for keyword rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// System prompt to guide AI behavior
    /// Command rules: optional (uses provider default if not set)
    /// Keyword rules: required (this is the main purpose of keyword rules)
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// Whether to strip the matched prefix from input before sending to AI
    /// Defaults to true for command rules, ignored for keyword rules
    #[serde(default)]
    pub strip_prefix: Option<bool>,

    // ===== Routing hints =====
    /// Intent type identifier (for logging and UI display)
    /// Examples: "translation", "research", "`code_generation`", "skills:build-macos-apps"
    /// Default: "general"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,

    /// Preferred model ID for this rule (optional)
    ///
    /// If specified, this model is used instead of automatic selection.
    /// Must be a valid model profile ID (e.g., "claude-opus", "gpt-4o").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,

    // ===== Command Mode Display fields =====
    /// SF Symbol icon name for command mode display
    /// Default: based on command type (bolt for Action, text.quote for Prompt)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl RoutingRuleConfig {
    /// Create a test config (for tests only)
    /// Note: This creates a command rule since it has an explicit provider
    #[must_use]
    pub fn test_config(regex: &str, provider: &str) -> Self {
        Self {
            rule_type: Some("command".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: Some(provider.to_string()),
            system_prompt: None,
            strip_prefix: None,
            intent_type: None,
            preferred_model: None,
            icon: None,
        }
    }

    /// Create a command rule config
    #[must_use]
    pub fn command(regex: &str, provider: &str, system_prompt: Option<&str>) -> Self {
        Self {
            rule_type: Some("command".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: Some(provider.to_string()),
            system_prompt: system_prompt.map(|s| s.to_string()),
            strip_prefix: Some(true),
            intent_type: None,
            preferred_model: None,
            icon: None,
        }
    }

    /// Create a keyword rule config
    #[must_use]
    pub fn keyword(regex: &str, system_prompt: &str) -> Self {
        Self {
            rule_type: Some("keyword".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: None,
            system_prompt: Some(system_prompt.to_string()),
            strip_prefix: None,
            intent_type: None,
            preferred_model: None,
            icon: None,
        }
    }

    /// Get the effective rule type (with auto-detection)
    #[must_use]
    pub fn get_rule_type(&self) -> &str {
        if let Some(ref rule_type) = self.rule_type {
            return rule_type.as_str();
        }
        // Auto-detect based on regex pattern
        if self.regex.starts_with("^/") {
            "command"
        } else {
            "keyword"
        }
    }

    /// Check if this is a command rule
    #[must_use]
    pub fn is_command_rule(&self) -> bool {
        self.get_rule_type() == "command"
    }

    /// Check if this is a keyword rule
    #[must_use]
    pub fn is_keyword_rule(&self) -> bool {
        self.get_rule_type() == "keyword"
    }

    /// Get intent type (with default value).
    ///
    /// `pub(crate)` — the wiring pass for the (currently-unimplemented)
    /// command-rule dispatch enum is still pending; kept crate-scoped so the
    /// public capture surface stays narrow.
    #[allow(dead_code)] // lib build doesn't compile `#[cfg(test)]`; the in-module test exercises this
    #[must_use]
    pub(crate) fn get_intent_type(&self) -> &str {
        self.intent_type.as_deref().unwrap_or("general")
    }

    /// Get preferred model ID.
    ///
    /// `pub(crate)` — see [`get_intent_type`](Self::get_intent_type).
    #[allow(dead_code)] // lib build doesn't compile `#[cfg(test)]`; the in-module test exercises this
    #[must_use]
    pub(crate) fn get_preferred_model(&self) -> Option<&str> {
        self.preferred_model.as_deref()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_intent_type_default() {
        let rule = RoutingRuleConfig::default();
        assert_eq!(rule.get_intent_type(), "general");
    }

    #[test]
    fn test_get_preferred_model() {
        let rule = RoutingRuleConfig {
            preferred_model: Some("claude-opus".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_preferred_model(), Some("claude-opus"));

        let rule_none = RoutingRuleConfig::default();
        assert_eq!(rule_none.get_preferred_model(), None);
    }
}
