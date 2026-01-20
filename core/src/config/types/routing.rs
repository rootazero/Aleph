//! Routing configuration types
//!
//! Contains routing rule configuration:
//! - RoutingRuleConfig: AI routing rules with command/keyword types
//!
//! # Model Router Integration
//!
//! This module integrates with the Model Router system for intelligent model selection.
//! The `intent_type` field is converted to `TaskIntent` which determines:
//! - Required model capabilities
//! - Optimal model selection via `ModelMatcher::route_by_intent()`
//!
//! Use `preferred_model` to override automatic model selection.

use crate::cowork::model_router::TaskIntent;
use serde::{Deserialize, Serialize};

// =============================================================================
// RoutingRuleConfig
// =============================================================================

/// Routing rule configuration for TOML parsing
///
/// Aether supports two types of routing rules:
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
/// - No `provider` field (uses default_provider)
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    // ===== Capability fields =====
    /// Required capabilities (e.g., ["memory", "search", "mcp"])
    /// Default: [] (no capabilities)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,

    /// Intent type identifier (for logging and UI display)
    /// Examples: "translation", "research", "code_generation", "skills:build-macos-apps"
    /// Default: "general"
    ///
    /// This is converted to `TaskIntent` via `get_task_intent()` for Model Router integration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,

    /// Preferred model ID for this rule (optional)
    ///
    /// If specified, this model is used instead of automatic selection via Model Router.
    /// Must be a valid model profile ID (e.g., "claude-opus", "gpt-4o").
    ///
    /// When both `provider` and `preferred_model` are set, `preferred_model` takes precedence
    /// for Model Router routing (provider is kept for backward compatibility).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,

    /// Context data injection format
    /// Options: "markdown", "xml", "json"
    /// Default: "markdown"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_format: Option<String>,

    // ===== Command Mode Display fields =====
    /// SF Symbol icon name for command mode display
    /// Default: based on command type (bolt for Action, text.quote for Prompt)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Short hint text for command mode display (max ~80px width)
    /// For builtin commands, this is overridden by localized hints
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,

    // ===== Natural Language Detection fields =====
    /// Trigger keywords for natural language command detection
    /// When user input contains any of these keywords, this command may be auto-invoked.
    /// Example: triggers = ["翻译", "translate", "转换语言"]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Vec<String>>,

    // ===== Skills fields (reserved) =====
    /// Skills ID (e.g., "build-macos-apps", "pdf")
    /// Only valid when intent_type = "skills:xxx"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,

    /// Skills version number (semantic versioning, e.g., "1.0.0")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_version: Option<String>,

    /// Skills workflow definition (JSON string)
    /// Example: '{"steps": [{"type": "tool_call", "tool": "read_files"}, ...]}'
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,

    /// Skills available tools list (JSON string array)
    /// Example: '["read_files", "write_files", "swift_compile"]'
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,

    /// Skills knowledge base path or URL
    /// Example: "~/.aether/skills/build-macos-apps/knowledge"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_base: Option<String>,
}

impl RoutingRuleConfig {
    /// Create a test config (for tests only)
    /// Note: This creates a command rule since it has an explicit provider
    pub fn test_config(regex: &str, provider: &str) -> Self {
        Self {
            rule_type: Some("command".to_string()), // Explicit command since provider is specified
            is_builtin: false,
            regex: regex.to_string(),
            provider: Some(provider.to_string()),
            system_prompt: None,
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            preferred_model: None,
            context_format: None,
            icon: None,
            hint: None,
            triggers: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        }
    }

    /// Create a command rule config
    pub fn command(regex: &str, provider: &str, system_prompt: Option<&str>) -> Self {
        Self {
            rule_type: Some("command".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: Some(provider.to_string()),
            system_prompt: system_prompt.map(|s| s.to_string()),
            strip_prefix: Some(true),
            capabilities: None,
            intent_type: None,
            preferred_model: None,
            context_format: None,
            icon: None,
            hint: None,
            triggers: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        }
    }

    /// Create a keyword rule config
    pub fn keyword(regex: &str, system_prompt: &str) -> Self {
        Self {
            rule_type: Some("keyword".to_string()),
            is_builtin: false,
            regex: regex.to_string(),
            provider: None,
            system_prompt: Some(system_prompt.to_string()),
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            preferred_model: None,
            context_format: None,
            icon: None,
            hint: None,
            triggers: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        }
    }

    /// Set the intent type (builder pattern)
    pub fn with_intent_type(mut self, intent_type: &str) -> Self {
        self.intent_type = Some(intent_type.to_string());
        self
    }

    /// Set the preferred model (builder pattern)
    ///
    /// The preferred model overrides automatic model selection via Model Router.
    pub fn with_preferred_model(mut self, model_id: &str) -> Self {
        self.preferred_model = Some(model_id.to_string());
        self
    }

    /// Get the effective rule type (with auto-detection)
    ///
    /// If `rule_type` is explicitly set, use it.
    /// Otherwise, auto-detect based on regex pattern:
    /// - Patterns starting with `^/` are command rules
    /// - Other patterns are keyword rules
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
    pub fn is_command_rule(&self) -> bool {
        self.get_rule_type() == "command"
    }

    /// Check if this is a keyword rule
    pub fn is_keyword_rule(&self) -> bool {
        self.get_rule_type() == "keyword"
    }

    /// Get provider name (required for command rules)
    ///
    /// For command rules, this returns the provider name.
    /// For keyword rules, this returns None (use default_provider).
    pub fn get_provider(&self) -> Option<&str> {
        if self.is_command_rule() {
            self.provider.as_deref()
        } else {
            None // Keyword rules don't specify provider
        }
    }

    /// Check if strip_prefix should be applied
    ///
    /// Command rules: defaults to true if not explicitly set
    /// Keyword rules: always false
    pub fn should_strip_prefix(&self) -> bool {
        if self.is_keyword_rule() {
            false
        } else {
            self.strip_prefix.unwrap_or(true)
        }
    }

    /// Get capabilities (with default value)
    pub fn get_capabilities(&self) -> Vec<crate::payload::Capability> {
        use crate::payload::Capability;

        self.capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .filter_map(|s| Capability::parse(s).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get intent type (with default value)
    pub fn get_intent_type(&self) -> &str {
        self.intent_type.as_deref().unwrap_or("general")
    }

    /// Get TaskIntent for Model Router integration
    ///
    /// Converts the `intent_type` string to a strongly-typed `TaskIntent` enum.
    /// This is the bridge between legacy routing rules and the Model Router.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let rule = RoutingRuleConfig {
    ///     intent_type: Some("code_generation".to_string()),
    ///     ..Default::default()
    /// };
    /// let intent = rule.get_task_intent();
    /// assert_eq!(intent, TaskIntent::CodeGeneration);
    /// ```
    pub fn get_task_intent(&self) -> TaskIntent {
        TaskIntent::from_string(self.get_intent_type())
    }

    /// Get preferred model ID for Model Router
    ///
    /// Returns the preferred model if explicitly set, otherwise None.
    /// When used with `ModelMatcher::route_by_intent_with_preference()`,
    /// this allows rules to override automatic model selection.
    pub fn get_preferred_model(&self) -> Option<&str> {
        self.preferred_model.as_deref()
    }

    /// Get context format (with default value)
    pub fn get_context_format(&self) -> crate::payload::ContextFormat {
        use crate::payload::ContextFormat;

        self.context_format
            .as_ref()
            .and_then(|s| ContextFormat::parse(s).ok())
            .unwrap_or(ContextFormat::Markdown)
    }

    // 🔮 Skills related helper methods (reserved for Solution C)

    /// Check if this is a Skills routing rule
    pub fn is_skills_rule(&self) -> bool {
        self.intent_type
            .as_ref()
            .map(|s| s.starts_with("skills:"))
            .unwrap_or(false)
    }

    /// Get Skills workflow definition (parse JSON)
    pub fn get_workflow_definition(&self) -> Option<serde_json::Value> {
        self.workflow
            .as_ref()
            .and_then(|json_str| serde_json::from_str(json_str).ok())
    }

    /// Get Skills tools list (parse JSON)
    pub fn get_tools_list(&self) -> Vec<String> {
        self.tools
            .as_ref()
            .and_then(|json_str| serde_json::from_str::<Vec<String>>(json_str).ok())
            .unwrap_or_default()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_task_intent_code_generation() {
        let rule = RoutingRuleConfig {
            intent_type: Some("code_generation".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_task_intent(), TaskIntent::CodeGeneration);
    }

    #[test]
    fn test_get_task_intent_code_aliases() {
        // Test various aliases
        let rule = RoutingRuleConfig {
            intent_type: Some("code".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_task_intent(), TaskIntent::CodeGeneration);

        let rule = RoutingRuleConfig {
            intent_type: Some("coding".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_task_intent(), TaskIntent::CodeGeneration);
    }

    #[test]
    fn test_get_task_intent_skills() {
        let rule = RoutingRuleConfig {
            intent_type: Some("skills:build-macos-apps".to_string()),
            ..Default::default()
        };
        let intent = rule.get_task_intent();
        assert!(matches!(intent, TaskIntent::Skills(ref id) if id == "build-macos-apps"));
    }

    #[test]
    fn test_get_task_intent_default() {
        let rule = RoutingRuleConfig::default();
        assert_eq!(rule.get_task_intent(), TaskIntent::GeneralChat);
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

    #[test]
    fn test_builder_with_intent_type() {
        let rule = RoutingRuleConfig::command("^/code", "anthropic", None)
            .with_intent_type("code_generation");
        assert_eq!(rule.get_task_intent(), TaskIntent::CodeGeneration);
    }

    #[test]
    fn test_builder_with_preferred_model() {
        let rule = RoutingRuleConfig::command("^/code", "anthropic", None)
            .with_preferred_model("claude-opus");
        assert_eq!(rule.get_preferred_model(), Some("claude-opus"));
    }

    #[test]
    fn test_builder_chain() {
        let rule = RoutingRuleConfig::command("^/translate", "anthropic", None)
            .with_intent_type("translation")
            .with_preferred_model("gpt-4o");

        assert_eq!(rule.get_task_intent(), TaskIntent::Translation);
        assert_eq!(rule.get_preferred_model(), Some("gpt-4o"));
    }

    #[test]
    fn test_task_intent_image_analysis() {
        let rule = RoutingRuleConfig {
            intent_type: Some("image_analysis".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_task_intent(), TaskIntent::ImageAnalysis);
    }

    #[test]
    fn test_task_intent_reasoning() {
        let rule = RoutingRuleConfig {
            intent_type: Some("reasoning".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_task_intent(), TaskIntent::Reasoning);

        // Test alias
        let rule = RoutingRuleConfig {
            intent_type: Some("think".to_string()),
            ..Default::default()
        };
        assert_eq!(rule.get_task_intent(), TaskIntent::Reasoning);
    }

    #[test]
    fn test_task_intent_custom() {
        let rule = RoutingRuleConfig {
            intent_type: Some("my_custom_workflow".to_string()),
            ..Default::default()
        };
        let intent = rule.get_task_intent();
        assert!(matches!(intent, TaskIntent::Custom(ref s) if s == "my_custom_workflow"));
    }

    #[test]
    fn test_routing_rule_with_triggers() {
        let toml = r#"
            regex = "^/translate"
            hint = "翻译文本"
            triggers = ["翻译", "translate", "转换语言"]
        "#;
        let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            rule.triggers,
            Some(vec![
                "翻译".to_string(),
                "translate".to_string(),
                "转换语言".to_string()
            ])
        );
    }
}
