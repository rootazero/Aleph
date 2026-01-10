//! Routing configuration types
//!
//! Contains routing rule configuration:
//! - RoutingRuleConfig: AI routing rules with command/keyword types

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,

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
            context_format: None,
            icon: None,
            hint: None,
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
            context_format: None,
            icon: None,
            hint: None,
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
            context_format: None,
            icon: None,
            hint: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        }
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
