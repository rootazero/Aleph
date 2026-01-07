use crate::error::{AetherError, Result};
/// Configuration structure for Aether
///
/// Phase 1: Stub implementation with basic fields.
/// Phase 4: Added memory configuration support.
/// Phase 5: Added AI provider configuration support.
/// Phase 6: Added Keychain integration and file watching support.
/// Phase 8: Added config file loading from ~/.config/aether/config.toml
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

// Submodules
pub mod watcher;
#[allow(unused_imports)]
pub use watcher::ConfigWatcher;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Default hotkey (hardcoded to "Command+Grave" in Phase 1)
    pub default_hotkey: String,
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
    /// Memory module configuration
    #[serde(default)]
    pub memory: MemoryConfig,
    /// AI provider configurations (Phase 5)
    /// Note: Not exposed through UniFFI dictionary, managed via separate methods
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderConfig>,
    /// Routing rules for smart AI provider selection (Phase 5)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RoutingRuleConfig>,
    /// Shortcuts configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<ShortcutsConfig>,
    /// Behavior configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<BehaviorConfig>,
    /// Search configuration (Search Capability Integration)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchConfigInternal>,
    /// Video transcript configuration (YouTube transcript extraction)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoConfig>,
    /// Trigger configuration (hotkey system refactor)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerConfig>,
    /// Smart conversation flow configuration
    #[serde(default)]
    pub smart_flow: SmartFlowConfig,
}

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
    /// Show command hints in command mode (default: true)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_command_hints: Option<bool>,
}

fn default_log_retention_days() -> u32 {
    7 // Keep logs for 7 days by default
}

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

fn default_summon_hotkey() -> String {
    "Command+Grave".to_string()
}

fn default_command_prompt_hotkey() -> String {
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
}

fn default_input_mode() -> String {
    "cut".to_string()
}

fn default_output_mode() -> String {
    "typewriter".to_string()
}

fn default_typing_speed() -> u32 {
    50 // 50 characters per second
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            input_mode: default_input_mode(),
            output_mode: default_output_mode(),
            typing_speed: default_typing_speed(),
            pii_scrubbing_enabled: false,
        }
    }
}

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

fn default_replace_hotkey() -> String {
    "DoubleTap+leftShift".to_string()
}

fn default_append_hotkey() -> String {
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

/// Provider config entry with name (for UniFFI)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigEntry {
    pub name: String,
    #[serde(flatten)]
    pub config: ProviderConfig,
}

/// Test connection result (for provider connection testing)
#[derive(Debug, Clone)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

/// Full configuration exposed through UniFFI
/// This wraps Config with a flattened provider list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullConfig {
    pub default_hotkey: String,
    pub general: GeneralConfig,
    pub memory: MemoryConfig,
    pub providers: Vec<ProviderConfigEntry>,
    pub rules: Vec<RoutingRuleConfig>,
    #[serde(default)]
    pub shortcuts: Option<ShortcutsConfig>,
    #[serde(default)]
    pub behavior: Option<BehaviorConfig>,
    #[serde(default)]
    pub search: Option<SearchConfig>,
    #[serde(default)]
    pub trigger: Option<TriggerConfig>,
}

impl From<Config> for FullConfig {
    fn from(config: Config) -> Self {
        let providers = config
            .providers
            .into_iter()
            .map(|(name, config)| ProviderConfigEntry { name, config })
            .collect();

        let search = config.search.map(|s| s.into());

        Self {
            default_hotkey: config.default_hotkey,
            general: config.general,
            memory: config.memory,
            providers,
            rules: config.rules,
            shortcuts: config.shortcuts,
            behavior: config.behavior,
            search,
            trigger: config.trigger,
        }
    }
}

/// Routing rule configuration for TOML parsing
///
/// Aether supports two types of routing rules:
///
/// ## Command Rules (指令规则)
/// - Pattern starts with `^/` (e.g., `^/draw`, `^/translate`)
/// - First-match-stops: only one command rule matches per request
/// - Requires `provider` field to specify which AI to use
/// - Command prefix is automatically stripped before sending to AI
///
/// ## Keyword Rules (关键词规则)
/// - Pattern does not start with `/` (e.g., `翻译成英文`, `代码优化`)
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
/// system_prompt = "请根据提示画一幅画"
///
/// # Keyword rule - prompt only, no provider
/// [[rules]]
/// rule_type = "keyword"
/// regex = "翻译成英文"
/// system_prompt = "翻译目标语言为英文"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    // ===== Rule Type (refactor-routing-rule-logic) =====
    /// Rule type: "command" or "keyword"
    /// - "command": Starts with /, first-match-stops, requires provider
    /// - "keyword": Non-/ pattern, all-match, prompt only
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

/// AI Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type: "openai", "claude", "gemini", "ollama", or custom name
    /// If not specified, inferred from provider name in config
    #[serde(default)]
    pub provider_type: Option<String>,
    /// API key for cloud providers (required for OpenAI, Claude, Gemini)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model name (e.g., "gpt-4o", "claude-3-5-sonnet-20241022", "gemini-3-flash", "llama3.2")
    pub model: String,
    /// Base URL for API endpoint (optional, defaults to official API)
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider brand color for UI (hex string, e.g., "#10a37f")
    #[serde(default = "default_provider_color")]
    pub color: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Whether the provider is enabled/active
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,

    // Common generation parameters
    /// Maximum tokens in response (optional)
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Temperature for response randomness (0.0-2.0 for OpenAI/Gemini, 0.0-1.0 for Claude)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling (0.0-1.0, optional)
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Top-k sampling (integer, optional, used by Claude, Gemini, Ollama)
    #[serde(default)]
    pub top_k: Option<u32>,

    // OpenAI-specific parameters
    /// Frequency penalty (-2.0 to 2.0, OpenAI only)
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0, OpenAI only)
    #[serde(default)]
    pub presence_penalty: Option<f32>,

    // Claude/Gemini/Ollama-specific parameters
    /// Stop sequences (comma-separated, Claude/Gemini/Ollama)
    #[serde(default)]
    pub stop_sequences: Option<String>,

    // Gemini-specific parameters
    /// Thinking level for Gemini 3 models (LOW or HIGH)
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// Media resolution for Gemini (LOW, MEDIUM, HIGH)
    #[serde(default)]
    pub media_resolution: Option<String>,

    // Ollama-specific parameters
    /// Repeat penalty for Ollama (default 1.1)
    #[serde(default)]
    pub repeat_penalty: Option<f32>,

    // System prompt handling mode
    /// How to send system prompts to the API:
    /// - "system" (default): Use a separate system message
    /// - "prepend": Prepend system prompt to user message (for APIs that ignore system role)
    #[serde(default)]
    pub system_prompt_mode: Option<String>,
}

fn default_provider_color() -> String {
    "#808080".to_string() // Gray as default
}

fn default_timeout_seconds() -> u64 {
    30 // 30 seconds default timeout
}

fn default_provider_enabled() -> bool {
    false // Providers are disabled by default, user must explicitly enable them
}

impl ProviderConfig {
    /// Infer provider type from config
    ///
    /// If `provider_type` is explicitly set, use it.
    /// Otherwise, infer from provider name:
    /// - "openai" -> "openai"
    /// - "claude" -> "claude"
    /// - "gemini" -> "gemini"
    /// - "ollama" -> "ollama"
    /// - anything with base_url -> "openai" (OpenAI-compatible)
    /// - default -> "openai"
    pub fn infer_provider_type(&self, provider_name: &str) -> String {
        if let Some(ref provider_type) = self.provider_type {
            return provider_type.clone();
        }

        // Infer from provider name
        let name_lower = provider_name.to_lowercase();
        if name_lower.contains("claude") {
            "claude".to_string()
        } else if name_lower.contains("gemini") || name_lower.contains("google") {
            "gemini".to_string()
        } else if name_lower.contains("ollama") {
            "ollama".to_string()
        } else {
            // Default to OpenAI-compatible (covers OpenAI, DeepSeek, Moonshot, etc.)
            "openai".to_string()
        }
    }

    /// Create a minimal test configuration with only required fields
    ///
    /// This is a helper for tests to avoid specifying all optional fields.
    /// All optional advanced parameters (like frequency_penalty, media_resolution, etc.) are set to None.
    pub fn test_config(model: impl Into<String>) -> Self {
        Self {
            provider_type: None,
            api_key: Some("test-key".to_string()),
            model: model.into(),
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: true, // Tests need enabled providers
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
        }
    }
}

/// Memory module configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Enable/disable memory module
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Embedding model name
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Maximum number of past interactions to retrieve
    #[serde(default = "default_max_context_items")]
    pub max_context_items: u32,
    /// Auto-delete memories older than N days (0 = never delete)
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Vector database backend: "sqlite-vec" or "lancedb"
    #[serde(default = "default_vector_db")]
    pub vector_db: String,
    /// Minimum similarity score to include memory (0.0-1.0)
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
    /// List of app bundle IDs to exclude from memory storage
    #[serde(default)]
    pub excluded_apps: Vec<String>,

    // AI-based memory retrieval settings
    /// Enable AI-based memory retrieval (replaces embedding similarity)
    #[serde(default = "default_ai_retrieval_enabled")]
    pub ai_retrieval_enabled: bool,
    /// Timeout for AI memory retrieval in milliseconds
    #[serde(default = "default_ai_retrieval_timeout_ms")]
    pub ai_retrieval_timeout_ms: u64,
    /// Maximum candidates to send to AI for selection
    #[serde(default = "default_ai_retrieval_max_candidates")]
    pub ai_retrieval_max_candidates: u32,
    /// Fallback count if AI selection fails
    #[serde(default = "default_ai_retrieval_fallback_count")]
    pub ai_retrieval_fallback_count: u32,
}

// Default value functions for MemoryConfig
fn default_enabled() -> bool {
    true
}

fn default_embedding_model() -> String {
    "all-MiniLM-L6-v2".to_string()
}

fn default_max_context_items() -> u32 {
    5
}

fn default_retention_days() -> u32 {
    90
}

fn default_vector_db() -> String {
    "sqlite-vec".to_string()
}

fn default_similarity_threshold() -> f32 {
    0.7 // Minimum similarity score for real embedding models
}

fn default_ai_retrieval_enabled() -> bool {
    true // Use AI-based memory retrieval by default
}

fn default_ai_retrieval_timeout_ms() -> u64 {
    3000 // 3 seconds timeout for AI memory selection
}

fn default_ai_retrieval_max_candidates() -> u32 {
    20 // Send up to 20 recent memories to AI for selection
}

fn default_ai_retrieval_fallback_count() -> u32 {
    3 // Return 3 most recent memories if AI fails
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            embedding_model: default_embedding_model(),
            max_context_items: default_max_context_items(),
            retention_days: default_retention_days(),
            vector_db: default_vector_db(),
            similarity_threshold: default_similarity_threshold(),
            excluded_apps: vec![
                "com.apple.keychainaccess".to_string(),
                "com.agilebits.onepassword7".to_string(),
                "com.lastpass.LastPass".to_string(),
                "com.bitwarden.desktop".to_string(),
            ],
            ai_retrieval_enabled: default_ai_retrieval_enabled(),
            ai_retrieval_timeout_ms: default_ai_retrieval_timeout_ms(),
            ai_retrieval_max_candidates: default_ai_retrieval_max_candidates(),
            ai_retrieval_fallback_count: default_ai_retrieval_fallback_count(),
        }
    }
}

/// Search module configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfigInternal {
    /// Enable/disable search functionality
    #[serde(default)]
    pub enabled: bool,

    /// Default search provider
    pub default_provider: String,

    /// Fallback providers (tried in order if default fails)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,

    /// Maximum number of results to return (default: 5)
    #[serde(default = "default_search_max_results")]
    pub max_results: usize,

    /// Search timeout in seconds (default: 10)
    #[serde(default = "default_search_timeout")]
    pub timeout_seconds: u64,

    /// Backend configurations
    pub backends: HashMap<String, SearchBackendConfig>,

    /// PII scrubbing configuration (migrate from behavior.pii_scrubbing_enabled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii: Option<PIIConfig>,
}

fn default_search_max_results() -> usize {
    5
}

fn default_search_max_results_u64() -> u64 {
    5
}

fn default_search_timeout() -> u64 {
    10
}

/// PII (Personally Identifiable Information) scrubbing configuration
///
/// Migrated from behavior.pii_scrubbing_enabled to search.pii
/// (integrate-search-registry proposal)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PIIConfig {
    /// Enable PII scrubbing (email, phone, SSN, etc.)
    #[serde(default)]
    pub enabled: bool,

    /// Scrub email addresses
    #[serde(default = "default_true")]
    pub scrub_email: bool,

    /// Scrub phone numbers
    #[serde(default = "default_true")]
    pub scrub_phone: bool,

    /// Scrub SSN (Social Security Numbers)
    #[serde(default = "default_true")]
    pub scrub_ssn: bool,

    /// Scrub credit card numbers
    #[serde(default = "default_true")]
    pub scrub_credit_card: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PIIConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scrub_email: true,
            scrub_phone: true,
            scrub_ssn: true,
            scrub_credit_card: true,
        }
    }
}

/// Smart conversation flow configuration
///
/// Controls intelligent intent detection and AI suggestion parsing
/// for a more natural conversation experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartFlowConfig {
    /// Enable/disable smart conversation flow
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Intent detection settings
    #[serde(default)]
    pub intent_detection: IntentDetectionConfig,
    /// AI suggestion parsing settings
    #[serde(default)]
    pub suggestion_parsing: SuggestionParsingConfig,
}

impl Default for SmartFlowConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            intent_detection: IntentDetectionConfig::default(),
            suggestion_parsing: SuggestionParsingConfig::default(),
        }
    }
}

/// Intent detection configuration
///
/// Controls which smart triggers are enabled for automatic capability invocation.
/// Each trigger detects specific patterns and can invoke builtin commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentDetectionConfig {
    /// Enable intent detection globally
    #[serde(default = "default_true")]
    pub enabled: bool,

    // AI-first intent detection (single-call architecture)
    /// Enable AI-first detection where AI decides if capability is needed
    /// When enabled, AI receives capability list and either responds directly
    /// or requests capability invocation via JSON
    #[serde(default = "default_false")]
    pub ai_first: bool,

    // AI-powered intent detection (legacy, language-agnostic)
    /// Use AI for intent detection (supports all languages)
    /// Note: When ai_first=true, this is ignored
    #[serde(default = "default_true")]
    pub use_ai: bool,
    /// Confidence threshold for AI detection (0.0 - 1.0)
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    /// Timeout for AI detection in milliseconds
    #[serde(default = "default_ai_timeout_ms")]
    pub ai_timeout_ms: u64,

    // Trigger enables
    /// Enable /search trigger (weather, news, general queries)
    #[serde(default = "default_true")]
    pub search: bool,
    /// Enable /video trigger (YouTube, Bilibili analysis)
    #[serde(default = "default_true")]
    pub video: bool,
    /// Enable /skill trigger (future)
    #[serde(default = "default_false")]
    pub skill: bool,
    /// Enable /mcp trigger (future)
    #[serde(default = "default_false")]
    pub mcp: bool,

    // Legacy fields for backward compatibility
    /// Legacy: weather intent (now part of search trigger)
    #[serde(default = "default_true")]
    pub weather: bool,
    /// Legacy: translation intent
    #[serde(default = "default_true")]
    pub translation: bool,
    /// Legacy: code help intent
    #[serde(default = "default_true")]
    pub code_help: bool,
}

fn default_confidence_threshold() -> f64 {
    0.7
}

fn default_ai_timeout_ms() -> u64 {
    3000
}

fn default_false() -> bool {
    false
}

impl Default for IntentDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // AI-first detection (new architecture)
            ai_first: false, // Disabled by default, opt-in
            // AI detection (legacy)
            use_ai: true,
            confidence_threshold: default_confidence_threshold(),
            ai_timeout_ms: default_ai_timeout_ms(),
            // Triggers
            search: true,
            video: true,
            skill: false,
            mcp: false,
            // Legacy
            weather: true,
            translation: true,
            code_help: true,
        }
    }
}

/// AI suggestion parsing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionParsingConfig {
    /// Enable suggestion parsing from AI responses
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum suggestions to extract
    #[serde(default = "default_max_suggestions")]
    pub max_suggestions: usize,
}

fn default_max_suggestions() -> usize {
    5
}

impl Default for SuggestionParsingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_suggestions: 5,
        }
    }
}

/// Search backend configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendConfig {
    /// Provider type: "tavily", "searxng", "brave", "google", "bing", "exa"
    pub provider_type: String,

    /// API key (required for most providers except SearXNG)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Base URL (required for SearXNG, optional for others)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Search engine ID (required for Google CSE only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
}

/// Search backend entry (name + config) - used for UniFFI serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendEntry {
    pub name: String,
    pub config: SearchBackendConfig,
}

/// Search configuration for UniFFI (backends as Vec instead of HashMap)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub enabled: bool,
    pub default_provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,
    #[serde(default = "default_search_max_results_u64")]
    pub max_results: u64,
    #[serde(default = "default_search_timeout")]
    pub timeout_seconds: u64,
    pub backends: Vec<SearchBackendEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii: Option<PIIConfig>,
}

impl From<SearchConfigInternal> for SearchConfig {
    fn from(config: SearchConfigInternal) -> Self {
        let backends = config
            .backends
            .into_iter()
            .map(|(name, config)| SearchBackendEntry { name, config })
            .collect();

        Self {
            enabled: config.enabled,
            default_provider: config.default_provider,
            fallback_providers: config.fallback_providers,
            max_results: config.max_results as u64,
            timeout_seconds: config.timeout_seconds,
            backends,
            pii: config.pii,
        }
    }
}

impl From<SearchConfig> for SearchConfigInternal {
    fn from(config: SearchConfig) -> Self {
        let backends = config
            .backends
            .into_iter()
            .map(|entry| (entry.name, entry.config))
            .collect();

        Self {
            enabled: config.enabled,
            default_provider: config.default_provider,
            fallback_providers: config.fallback_providers,
            max_results: config.max_results as usize,
            timeout_seconds: config.timeout_seconds,
            backends,
            pii: config.pii,
        }
    }
}

/// Video transcript extraction configuration
///
/// Enables extracting transcripts from video platforms (currently YouTube)
/// and injecting them into the AI context for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    /// Enable video transcript extraction
    #[serde(default = "default_video_enabled")]
    pub enabled: bool,

    /// Enable YouTube transcript extraction
    #[serde(default = "default_youtube_transcript")]
    pub youtube_transcript: bool,

    /// Preferred language for transcripts (ISO 639-1 code, e.g., "en", "zh")
    #[serde(default = "default_preferred_language")]
    pub preferred_language: String,

    /// Maximum transcript length in characters (0 = no limit)
    #[serde(default = "default_max_transcript_length")]
    pub max_transcript_length: usize,
}

fn default_video_enabled() -> bool {
    true
}

fn default_youtube_transcript() -> bool {
    true
}

fn default_preferred_language() -> String {
    "en".to_string()
}

fn default_max_transcript_length() -> usize {
    50000 // ~12,500 words, roughly 25-30 minutes of video
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            enabled: default_video_enabled(),
            youtube_transcript: default_youtube_transcript(),
            preferred_language: default_preferred_language(),
            max_transcript_length: default_max_transcript_length(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_hotkey: "Grave".to_string(), // Single ` key (backtick) for quick access
            general: GeneralConfig::default(),
            memory: MemoryConfig::default(),
            providers: HashMap::new(),
            // Preset routing rules for builtin commands (add-search-settings-ui)
            rules: vec![
                // /search command - web search capability
                RoutingRuleConfig {
                    rule_type: Some("command".to_string()),
                    is_builtin: true,
                    regex: r"^/search\s+".to_string(),
                    provider: Some("openai".to_string()), // Default, user can override
                    system_prompt: Some("You are a helpful search assistant. Use the search capability to find up-to-date information.".to_string()),
                    strip_prefix: Some(true),
                    capabilities: Some(vec!["search".to_string()]),
                    intent_type: Some("builtin_search".to_string()),
                    context_format: Some("markdown".to_string()),
                    skill_id: None,
                    skill_version: None,
                    workflow: None,
                    tools: None,
                    knowledge_base: None,
                    icon: None,
                    hint: None,
                },
                // /mcp command - Model Context Protocol integration (reserved for future)
                RoutingRuleConfig {
                    rule_type: Some("command".to_string()),
                    is_builtin: true,
                    regex: r"^/mcp\s+".to_string(),
                    provider: Some("openai".to_string()),
                    system_prompt: Some("You are an MCP integration assistant. (Feature not yet implemented)".to_string()),
                    strip_prefix: Some(true),
                    capabilities: None, // Will add ["mcp"] when implemented
                    intent_type: Some("builtin_mcp".to_string()),
                    context_format: Some("markdown".to_string()),
                    skill_id: None,
                    skill_version: None,
                    workflow: None,
                    tools: None,
                    knowledge_base: None,
                    icon: None,
                    hint: None,
                },
                // /skill command - Skills workflow execution (reserved for future)
                RoutingRuleConfig {
                    rule_type: Some("command".to_string()),
                    is_builtin: true,
                    regex: r"^/skill\s+".to_string(),
                    provider: Some("openai".to_string()),
                    system_prompt: Some("You are a skills execution assistant. (Feature not yet implemented)".to_string()),
                    strip_prefix: Some(true),
                    capabilities: None, // Will add ["skills"] when implemented
                    intent_type: Some("skills".to_string()),
                    context_format: Some("markdown".to_string()),
                    skill_id: None,
                    skill_version: None,
                    workflow: None,
                    tools: None,
                    knowledge_base: None,
                    icon: None,
                    hint: None,
                },
                // /video command - YouTube video transcript analysis
                RoutingRuleConfig {
                    rule_type: Some("command".to_string()),
                    is_builtin: true,
                    regex: r"^/video\s+".to_string(),
                    provider: Some("openai".to_string()), // Default, user can override
                    system_prompt: Some("You are a video content analyst. A video transcript will be provided in the context section below if available. Analyze the transcript and provide insights, summaries, or answer questions about the video content. If no transcript is provided, explain that the video may not have captions enabled or transcript extraction failed.".to_string()),
                    strip_prefix: Some(true),
                    capabilities: Some(vec!["video".to_string(), "memory".to_string()]),
                    intent_type: Some("video_analysis".to_string()),
                    context_format: Some("markdown".to_string()),
                    skill_id: None,
                    skill_version: None,
                    workflow: None,
                    tools: None,
                    knowledge_base: None,
                    icon: None,
                    hint: None,
                },
            ],
            shortcuts: Some(ShortcutsConfig::default()),
            behavior: Some(BehaviorConfig::default()),
            search: None,
            video: Some(VideoConfig::default()),
            trigger: Some(TriggerConfig::default()),
            smart_flow: SmartFlowConfig::default(),
        }
    }
}

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the default config path: ~/.config/aether/config.toml
    pub fn default_path() -> PathBuf {
        if let Some(home) = dirs::home_dir() {
            home.join(".config").join("aether").join("config.toml")
        } else {
            // Fallback to current directory if home dir not found
            PathBuf::from("config.toml")
        }
    }

    /// Load configuration from a TOML file
    ///
    /// # Arguments
    /// * `path` - Path to the config file
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config
    /// * `Err(AetherError::ConfigNotFound)` - File doesn't exist
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```no_run
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load_from_file("config.toml").unwrap();
    /// ```
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        debug!(path = %path.display(), "Attempting to load config from file");

        // Check if file exists
        if !path.exists() {
            error!(path = %path.display(), "Config file not found");
            return Err(AetherError::invalid_config(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to read config file");
            AetherError::invalid_config(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config file read successfully, parsing TOML"
        );

        // Parse TOML
        let mut config: Config = toml::from_str(&contents).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to parse config TOML");
            AetherError::invalid_config(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            "Config parsed successfully, merging builtin rules"
        );

        // Merge builtin rules with user rules
        // Builtin rules (/search, /mcp, /skill) should be prepended to user rules
        // unless user has defined a rule with the same regex pattern
        config.merge_builtin_rules();

        debug!(
            path = %path.display(),
            rules_count = config.rules.len(),
            "Builtin rules merged, checking for migrations"
        );

        // Migrate PII config from behavior to search (integrate-search-registry)
        let pii_migrated = config.migrate_pii_config();
        if pii_migrated {
            info!("Migrated PII config from behavior.pii_scrubbing_enabled to search.pii.enabled");
        }

        // Migrate input_mode to trigger config (hotkey-system-refactor)
        let trigger_migrated = config.migrate_trigger_config();
        if trigger_migrated {
            info!("Migrated input_mode config to new trigger config");
        }

        // Auto-save if any migration was performed
        if pii_migrated || trigger_migrated {
            if let Err(e) = config.save() {
                warn!(error = %e, "Failed to auto-save migrated config");
            }
        }

        // Validate config
        config.validate()?;

        info!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            memory_enabled = config.memory.enabled,
            "Config loaded and validated successfully"
        );

        Ok(config)
    }

    /// Load configuration from default path (~/.config/aether/config.toml)
    /// Falls back to default config if file doesn't exist
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config or default config
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```no_run
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load().unwrap();
    /// ```
    pub fn load() -> Result<Self> {
        let path = Self::default_path();

        debug!(path = %path.display(), "Loading config from default path");

        if path.exists() {
            info!(path = %path.display(), "Found config file, loading");
            Self::load_from_file(&path)
        } else {
            info!(
                path = %path.display(),
                "Config file not found, using default configuration"
            );
            Ok(Self::default())
        }
    }

    /// Get builtin routing rules
    ///
    /// Returns the preset routing rules for builtin commands (/search, /mcp, /skill).
    /// These rules are always available and merged with user-defined rules.
    fn builtin_rules() -> Vec<RoutingRuleConfig> {
        vec![
            // /search command - web search capability
            RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: r"^/search\s+".to_string(),
                provider: Some("openai".to_string()), // Default, actual provider determined by default_provider
                system_prompt: Some("You are a helpful search assistant. Answer questions based on the web search results provided below. Be concise and cite sources when possible.".to_string()),
                strip_prefix: Some(true),
                capabilities: Some(vec!["search".to_string()]),
                intent_type: Some("builtin_search".to_string()),
                context_format: Some("markdown".to_string()),
                skill_id: None,
                skill_version: None,
                workflow: None,
                tools: None,
                knowledge_base: None,
                icon: None,
                hint: None,
            },
            // /mcp command - Model Context Protocol integration (reserved for future)
            RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: r"^/mcp\s+".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("You are an MCP integration assistant. (Feature not yet implemented)".to_string()),
                strip_prefix: Some(true),
                capabilities: None, // Will add ["mcp"] when implemented
                intent_type: Some("builtin_mcp".to_string()),
                context_format: Some("markdown".to_string()),
                skill_id: None,
                skill_version: None,
                workflow: None,
                tools: None,
                knowledge_base: None,
                icon: None,
                hint: None,
            },
            // /skill command - Skills workflow execution (reserved for future)
            RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: r"^/skill\s+".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("You are a skills execution assistant. (Feature not yet implemented)".to_string()),
                strip_prefix: Some(true),
                capabilities: None, // Will add ["skills"] when implemented
                intent_type: Some("skills".to_string()),
                context_format: Some("markdown".to_string()),
                skill_id: None,
                skill_version: None,
                workflow: None,
                tools: None,
                knowledge_base: None,
                icon: None,
                hint: None,
            },
        ]
    }

    /// Merge builtin rules with user-defined rules
    ///
    /// Builtin rules (/search, /mcp, /skill) are prepended to user rules
    /// unless the user has defined a rule with the same regex pattern.
    /// This ensures builtin commands always work while allowing user overrides.
    ///
    /// Builtin rules will use the user's default_provider if configured,
    /// otherwise fall back to the first configured provider.
    fn merge_builtin_rules(&mut self) {
        let mut builtin = Self::builtin_rules();

        // Determine the provider to use for builtin rules
        // Priority: default_provider > first configured provider > "openai" (fallback)
        let builtin_provider = self
            .general
            .default_provider
            .clone()
            .or_else(|| self.providers.keys().next().cloned())
            .unwrap_or_else(|| "openai".to_string());

        // Update builtin rules to use the resolved provider
        for rule in &mut builtin {
            rule.provider = Some(builtin_provider.clone());
        }

        // Collect user regex patterns as owned strings to avoid borrowing issues
        let user_regexes: std::collections::HashSet<String> = self
            .rules
            .iter()
            .map(|r| r.regex.clone())
            .collect();

        let user_count = user_regexes.len();

        // Prepend builtin rules that user hasn't overridden
        let mut merged_rules: Vec<RoutingRuleConfig> = builtin
            .into_iter()
            .filter(|r| !user_regexes.contains(&r.regex))
            .collect();

        // Take ownership of user rules and append
        let user_rules = std::mem::take(&mut self.rules);
        merged_rules.extend(user_rules);
        self.rules = merged_rules;

        debug!(
            builtin_count = Self::builtin_rules().len(),
            user_count = user_count,
            merged_count = self.rules.len(),
            builtin_provider = %builtin_provider,
            "Merged builtin rules with user rules"
        );
    }

    /// Validate configuration
    ///
    /// Checks:
    /// - Provider references in rules exist in providers map
    /// - Default provider exists (if specified)
    /// - API keys are present for cloud providers
    /// - Regex patterns are valid
    pub fn validate(&self) -> Result<()> {
        debug!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Starting config validation"
        );

        // Warn if no default provider is configured
        if self.general.default_provider.is_none() {
            warn!(
                "No default_provider configured. \
                 Requests will fail if no routing rule matches. \
                 Recommendation: Set general.default_provider in config"
            );
        }

        // Warn if no routing rules are configured
        if self.rules.is_empty() {
            warn!(
                "No routing rules configured. \
                 All requests will use default_provider (if set). \
                 Recommendation: Add routing rules to enable context-aware routing"
            );
        }

        // Validate default provider exists (if configured)
        if let Some(ref default_provider) = self.general.default_provider {
            if !self.providers.contains_key(default_provider) {
                error!(default_provider = %default_provider, "Default provider not found");
                return Err(AetherError::invalid_config(format!(
                    "Default provider '{}' not found in providers",
                    default_provider
                )));
            }
            debug!(default_provider = %default_provider, "Default provider validated");
        }

        // Validate provider configurations
        for (name, provider) in &self.providers {
            let provider_type = provider.infer_provider_type(name);

            // Check API key for cloud providers (not required for Ollama)
            if (provider_type == "openai" || provider_type == "claude" || provider_type == "gemini")
                && provider.api_key.is_none()
            {
                error!(provider = %name, provider_type = %provider_type, "Provider missing API key");
                return Err(AetherError::invalid_config(format!(
                    "Provider '{}' requires an API key",
                    name
                )));
            }

            // Validate timeout
            if provider.timeout_seconds == 0 {
                error!(provider = %name, "Provider timeout is zero");
                return Err(AetherError::invalid_config(format!(
                    "Provider '{}' timeout must be greater than 0",
                    name
                )));
            }

            // Validate temperature if specified (provider-specific ranges)
            if let Some(temp) = provider.temperature {
                let (min, max, provider_name): (f32, f32, &str) = match provider_type.as_str() {
                    "claude" => (0.0, 1.0, "Claude"),
                    "openai" => (0.0, 2.0, "OpenAI"),
                    "gemini" => (0.0, 2.0, "Gemini"),
                    "ollama" => (0.0, f32::MAX, "Ollama"),
                    _ => (0.0, 2.0, "Custom"),
                };

                if !(min..=max).contains(&temp) {
                    error!(provider = %name, temperature = temp, "Invalid temperature for {}", provider_name);
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' ({}) temperature must be between {} and {}, got {}",
                        name, provider_name, min, max, temp
                    )));
                }
            }

            // Validate max_tokens if specified
            if let Some(max_tokens) = provider.max_tokens {
                if max_tokens == 0 {
                    error!(provider = %name, max_tokens = max_tokens, "Invalid max_tokens");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' max_tokens must be greater than 0, got {}",
                        name, max_tokens
                    )));
                }
            }

            // Validate top_p if specified
            if let Some(top_p) = provider.top_p {
                if !(0.0..=1.0).contains(&top_p) {
                    error!(provider = %name, top_p = top_p, "Invalid top_p");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' top_p must be between 0.0 and 1.0, got {}",
                        name, top_p
                    )));
                }
            }

            // Validate top_k if specified
            if let Some(top_k) = provider.top_k {
                if top_k == 0 {
                    error!(provider = %name, top_k = top_k, "Invalid top_k");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' top_k must be greater than 0, got {}",
                        name, top_k
                    )));
                }
            }

            // Validate OpenAI-specific parameters
            if provider_type == "openai" {
                if let Some(freq_pen) = provider.frequency_penalty {
                    if !(-2.0..=2.0).contains(&freq_pen) {
                        error!(provider = %name, frequency_penalty = freq_pen, "Invalid frequency_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' frequency_penalty must be between -2.0 and 2.0, got {}",
                            name, freq_pen
                        )));
                    }
                }

                if let Some(pres_pen) = provider.presence_penalty {
                    if !(-2.0..=2.0).contains(&pres_pen) {
                        error!(provider = %name, presence_penalty = pres_pen, "Invalid presence_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' presence_penalty must be between -2.0 and 2.0, got {}",
                            name, pres_pen
                        )));
                    }
                }
            }

            // Validate Gemini-specific parameters
            if provider_type == "gemini" {
                if let Some(ref thinking_level) = provider.thinking_level {
                    if thinking_level != "LOW" && thinking_level != "HIGH" {
                        error!(provider = %name, thinking_level = %thinking_level, "Invalid thinking_level");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' thinking_level must be 'LOW' or 'HIGH', got '{}'",
                            name, thinking_level
                        )));
                    }
                }

                if let Some(ref media_res) = provider.media_resolution {
                    if media_res != "LOW" && media_res != "MEDIUM" && media_res != "HIGH" {
                        error!(provider = %name, media_resolution = %media_res, "Invalid media_resolution");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' media_resolution must be 'LOW', 'MEDIUM', or 'HIGH', got '{}'",
                            name, media_res
                        )));
                    }
                }
            }

            // Validate Ollama-specific parameters
            if provider_type == "ollama" {
                if let Some(repeat_pen) = provider.repeat_penalty {
                    if repeat_pen < 0.0 {
                        error!(provider = %name, repeat_penalty = repeat_pen, "Invalid repeat_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' repeat_penalty must be >= 0.0, got {}",
                            name, repeat_pen
                        )));
                    }
                }
            }

            debug!(
                provider = %name,
                provider_type = %provider_type,
                timeout_seconds = provider.timeout_seconds,
                "Provider validated"
            );
        }

        // Validate routing rules
        for (idx, rule) in self.rules.iter().enumerate() {
            let rule_type = rule.get_rule_type();

            // Command rules require a provider (skip for builtin rules which use default_provider)
            if rule.is_command_rule() && !rule.is_builtin {
                match &rule.provider {
                    Some(provider) => {
                        if !self.providers.contains_key(provider) {
                            error!(
                                rule_index = idx + 1,
                                provider = %provider,
                                "Command rule references unknown provider"
                            );
                            return Err(AetherError::invalid_config(format!(
                                "Command rule #{} references unknown provider '{}'",
                                idx + 1,
                                provider
                            )));
                        }
                    }
                    None => {
                        error!(
                            rule_index = idx + 1,
                            regex = %rule.regex,
                            "Command rule missing provider"
                        );
                        return Err(AetherError::invalid_config(format!(
                            "Command rule #{} (regex: '{}') requires a provider",
                            idx + 1,
                            rule.regex
                        )));
                    }
                }
            }

            // Keyword rules require a system_prompt
            if rule.is_keyword_rule() && rule.system_prompt.is_none() {
                warn!(
                    rule_index = idx + 1,
                    regex = %rule.regex,
                    "Keyword rule missing system_prompt - rule will have no effect"
                );
            }

            debug!(
                rule_index = idx + 1,
                rule_type = %rule_type,
                regex = %rule.regex,
                is_builtin = rule.is_builtin,
                "Validating rule"
            );

            // Validate regex pattern
            if let Err(e) = regex::Regex::new(&rule.regex) {
                error!(
                    rule_index = idx + 1,
                    regex = %rule.regex,
                    error = %e,
                    "Invalid regex pattern"
                );
                return Err(AetherError::invalid_config(format!(
                    "Rule #{} has invalid regex '{}': {}",
                    idx + 1,
                    rule.regex,
                    e
                )));
            }
        }

        // Validate memory config
        if self.memory.max_context_items == 0 {
            error!("Memory max_context_items is zero");
            return Err(AetherError::invalid_config(
                "memory.max_context_items must be greater than 0",
            ));
        }

        if !(0.0..=1.0).contains(&self.memory.similarity_threshold) {
            error!(
                threshold = self.memory.similarity_threshold,
                "Invalid similarity threshold"
            );
            return Err(AetherError::invalid_config(format!(
                "memory.similarity_threshold must be between 0.0 and 1.0, got {}",
                self.memory.similarity_threshold
            )));
        }

        debug!(
            memory_enabled = self.memory.enabled,
            max_context_items = self.memory.max_context_items,
            similarity_threshold = self.memory.similarity_threshold,
            "Memory config validated"
        );

        // Validate language preference
        if let Some(ref language) = self.general.language {
            // List of supported language codes (must match .lproj directory names)
            let supported_languages = vec!["en", "zh-Hans"];

            if !supported_languages.contains(&language.as_str()) {
                tracing::warn!(
                    language = %language,
                    supported = ?supported_languages,
                    "Invalid language code '{}', falling back to system language. Supported languages: {:?}",
                    language,
                    supported_languages
                );
            } else {
                debug!(language = %language, "Language preference validated");
            }
        }

        // Validate search configuration
        if let Some(ref search_config) = self.search {
            if search_config.enabled {
                // Validate default provider exists
                if !search_config
                    .backends
                    .contains_key(&search_config.default_provider)
                {
                    error!(
                        default_provider = %search_config.default_provider,
                        "Search default provider not found in backends"
                    );
                    return Err(AetherError::invalid_config(format!(
                        "Search default provider '{}' not found in backends",
                        search_config.default_provider
                    )));
                }

                // Validate fallback providers exist
                if let Some(ref fallback_providers) = search_config.fallback_providers {
                    for provider_name in fallback_providers {
                        if !search_config.backends.contains_key(provider_name) {
                            error!(
                                fallback_provider = %provider_name,
                                "Search fallback provider not found in backends"
                            );
                            return Err(AetherError::invalid_config(format!(
                                "Search fallback provider '{}' not found in backends",
                                provider_name
                            )));
                        }
                    }
                }

                // Validate max_results is reasonable
                if search_config.max_results == 0 {
                    error!("Search max_results cannot be 0");
                    return Err(AetherError::invalid_config(
                        "Search max_results must be greater than 0".to_string(),
                    ));
                }

                if search_config.max_results > 100 {
                    warn!(
                        max_results = search_config.max_results,
                        "Search max_results is very high (>100), this may impact performance"
                    );
                }

                // Validate timeout is reasonable
                if search_config.timeout_seconds == 0 {
                    error!("Search timeout cannot be 0");
                    return Err(AetherError::invalid_config(
                        "Search timeout_seconds must be greater than 0".to_string(),
                    ));
                }

                // Validate each backend configuration
                for (backend_name, backend_config) in &search_config.backends {
                    let provider_type = backend_config.provider_type.as_str();

                    match provider_type {
                        "tavily" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Tavily backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Tavily) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "brave" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Brave backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Brave) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "google" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Google backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Google) requires an API key",
                                    backend_name
                                )));
                            }
                            if backend_config.engine_id.is_none() {
                                error!(backend = %backend_name, "Google backend requires engine_id");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Google) requires an engine_id",
                                    backend_name
                                )));
                            }
                        }
                        "bing" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Bing backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Bing) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "exa" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Exa backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Exa) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "searxng" => {
                            if backend_config.base_url.is_none() {
                                error!(backend = %backend_name, "SearXNG backend requires base_url");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (SearXNG) requires a base_url",
                                    backend_name
                                )));
                            }
                        }
                        _ => {
                            warn!(
                                backend = %backend_name,
                                provider_type = %provider_type,
                                "Unknown search provider type"
                            );
                        }
                    }

                    debug!(
                        backend = %backend_name,
                        provider_type = %provider_type,
                        "Search backend validated"
                    );
                }

                debug!(
                    enabled = search_config.enabled,
                    default_provider = %search_config.default_provider,
                    backends_count = search_config.backends.len(),
                    "Search config validated"
                );
            }
        }

        info!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Config validation completed successfully"
        );

        Ok(())
    }

    /// Save configuration to a TOML file with atomic write
    ///
    /// This method uses atomic write operation to prevent corruption:
    /// 1. Write to temporary file (.tmp suffix)
    /// 2. fsync() to ensure data is on disk
    /// 3. Atomic rename to target path
    ///
    /// This ensures that the config file is never in a partially written state,
    /// even if the application crashes or loses power during the write.
    ///
    /// # Arguments
    /// * `path` - Target path for config file
    ///
    /// # Errors
    /// * `AetherError::InvalidConfig` - Failed to serialize or write config
    ///
    /// # Example
    /// ```no_run
    /// let config = Config::default();
    /// config.save_to_file("config.toml")?;
    /// ```
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        debug!(
            path = %path.display(),
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Attempting to save config"
        );

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                error!(directory = %parent.display(), error = %e, "Failed to create config directory");
                AetherError::invalid_config(format!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
            debug!(directory = %parent.display(), "Config directory ensured");
        }

        // Serialize to TOML
        let contents = toml::to_string_pretty(self).map_err(|e| {
            error!(error = %e, "Failed to serialize config to TOML");
            AetherError::invalid_config(format!("Failed to serialize config: {}", e))
        })?;

        debug!(
            size_bytes = contents.len(),
            lines = contents.lines().count(),
            "Config serialized to TOML"
        );

        // Create temporary file in the same directory (atomic rename requirement)
        let temp_path = path.with_extension("tmp");

        // Write to temp file
        fs::write(&temp_path, &contents).map_err(|e| {
            error!(temp_path = %temp_path.display(), error = %e, "Failed to write temp file");
            AetherError::invalid_config(format!(
                "Failed to write temp config file {}: {}",
                temp_path.display(),
                e
            ))
        })?;

        debug!(temp_path = %temp_path.display(), "Wrote config to temp file");

        // fsync the temp file to ensure data is on disk
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    error!(temp_path = %temp_path.display(), error = %e, "Failed to open temp file for fsync");
                    AetherError::invalid_config(format!(
                        "Failed to open temp file for fsync: {}",
                        e
                    ))
                })?;

            // Sync file data and metadata
            file.sync_all().map_err(|e| {
                error!(temp_path = %temp_path.display(), error = %e, "Failed to fsync temp file");
                AetherError::invalid_config(format!("Failed to fsync temp file: {}", e))
            })?;

            debug!(temp_path = %temp_path.display(), "Fsynced temp file to disk");
        }

        // Atomic rename (overwrites target if exists)
        fs::rename(&temp_path, path).map_err(|e| {
            error!(
                temp_path = %temp_path.display(),
                target_path = %path.display(),
                error = %e,
                "Failed to atomically rename temp file"
            );
            // Clean up temp file on error
            let _ = fs::remove_file(&temp_path);
            AetherError::invalid_config(format!(
                "Failed to rename temp config to {}: {}",
                path.display(),
                e
            ))
        })?;

        // Set file permissions to 600 (owner read/write only) for security
        // This protects API keys stored in the config file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)
                .map_err(|e| {
                    error!(path = %path.display(), error = %e, "Failed to get file metadata");
                    AetherError::invalid_config(format!("Failed to get file metadata: {}", e))
                })?
                .permissions();
            perms.set_mode(0o600); // Owner read/write only
            fs::set_permissions(path, perms).map_err(|e| {
                error!(path = %path.display(), error = %e, "Failed to set file permissions to 600");
                AetherError::invalid_config(format!("Failed to set file permissions: {}", e))
            })?;
            debug!(path = %path.display(), "Set file permissions to 600 (owner read/write only)");
        }

        info!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config saved successfully with atomic write"
        );

        Ok(())
    }

    /// Save configuration to default path with atomic write
    ///
    /// This is a convenience method that saves to ~/.config/aether/config.toml
    /// using atomic write operation.
    ///
    /// # Example
    /// ```no_run
    /// let mut config = Config::default();
    /// config.default_hotkey = "Command+Shift+A".to_string();
    /// config.save()?;
    /// ```
    pub fn save(&self) -> Result<()> {
        self.save_to_file(Self::default_path())
    }

    /// Migrate PII config from behavior to search (integrate-search-registry)
    ///
    /// This method performs automatic migration of PII settings:
    /// 1. Detects old `behavior.pii_scrubbing_enabled` field
    /// 2. Creates `search.pii.enabled` if missing
    /// 3. Removes old field from behavior config
    ///
    /// # Returns
    /// * `true` - Migration was performed
    /// * `false` - No migration needed (already migrated or no old config)
    fn migrate_pii_config(&mut self) -> bool {
        // Check if migration is needed
        let needs_migration = if let Some(ref behavior) = self.behavior {
            // Old field exists, check if new config is missing
            behavior.pii_scrubbing_enabled
                && self.search.as_ref().and_then(|s| s.pii.as_ref()).is_none()
        } else {
            false
        };

        if !needs_migration {
            return false;
        }

        // Get old PII value
        let pii_enabled = self
            .behavior
            .as_ref()
            .map(|b| b.pii_scrubbing_enabled)
            .unwrap_or(false);

        debug!(
            pii_enabled = pii_enabled,
            "Migrating PII config from behavior to search"
        );

        // Initialize search config if missing
        if self.search.is_none() {
            self.search = Some(SearchConfigInternal {
                enabled: false,
                default_provider: String::new(),
                fallback_providers: None,
                max_results: 5,
                timeout_seconds: 10,
                backends: HashMap::new(),
                pii: None,
            });
        }

        // Set PII config in search
        if let Some(ref mut search_config) = self.search {
            search_config.pii = Some(PIIConfig {
                enabled: pii_enabled,
                ..Default::default()
            });
        }

        // Remove old field from behavior (by replacing with default)
        if let Some(ref mut behavior) = self.behavior {
            behavior.pii_scrubbing_enabled = false;
        }

        true
    }

    /// Migrate from old config to new trigger config
    ///
    /// Sets default replace/append hotkeys if trigger config doesn't exist.
    ///
    /// Returns true if migration was performed
    fn migrate_trigger_config(&mut self) -> bool {
        // Check if migration is needed
        if self.trigger.is_some() {
            return false;
        }

        debug!("Migrating to new trigger config with default hotkeys");

        // Create trigger config with defaults
        self.trigger = Some(TriggerConfig {
            replace_hotkey: default_replace_hotkey(),
            append_hotkey: default_append_hotkey(),
        });

        true
    }

    /// Get the default provider if it exists and is enabled
    ///
    /// Returns None if:
    /// - No default provider is configured
    /// - Default provider does not exist in providers map
    /// - Default provider is disabled
    ///
    /// # Returns
    /// * `Some(String)` - The name of the enabled default provider
    /// * `None` - No valid default provider
    pub fn get_default_provider(&self) -> Option<String> {
        self.general.default_provider.as_ref().and_then(|name| {
            self.providers.get(name).and_then(|config| {
                if config.enabled {
                    Some(name.clone())
                } else {
                    None
                }
            })
        })
    }

    /// Set the default provider with validation
    ///
    /// Validates that:
    /// - Provider exists in providers map
    /// - Provider is enabled
    ///
    /// # Arguments
    /// * `name` - The name of the provider to set as default
    ///
    /// # Returns
    /// * `Ok(())` - Successfully set default provider
    /// * `Err(AetherError::InvalidConfig)` - Provider not found or disabled
    pub fn set_default_provider(&mut self, name: &str) -> Result<()> {
        match self.providers.get(name) {
            Some(config) if config.enabled => {
                debug!(provider = %name, "Setting default provider");
                self.general.default_provider = Some(name.to_string());
                Ok(())
            }
            Some(_) => {
                error!(provider = %name, "Cannot set disabled provider as default");
                Err(AetherError::invalid_config(format!(
                    "Provider '{}' is not enabled",
                    name
                )))
            }
            None => {
                error!(provider = %name, "Provider not found in config");
                Err(AetherError::invalid_config(format!(
                    "Provider '{}' not found",
                    name
                )))
            }
        }
    }

    /// Get list of all enabled provider names
    ///
    /// Returns provider names in alphabetical order
    ///
    /// # Returns
    /// * `Vec<String>` - List of enabled provider names
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self
            .providers
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        providers.sort();
        providers
    }

    // ROUTING RULE MANAGEMENT METHODS

    /// Add a new routing rule at the top of the list (highest priority)
    ///
    /// New rules are inserted at index 0 to give them the highest priority
    /// in the first-match-stops routing algorithm.
    ///
    /// # Arguments
    /// * `rule` - The routing rule configuration to add
    ///
    /// # Example
    /// ```rust,no_run
    /// # use aethecore::config::{Config, RoutingRuleConfig};
    /// let mut config = Config::default();
    /// config.add_rule_at_top(RoutingRuleConfig {
    ///     regex: r"^\[VSCode\]".to_string(),
    ///     provider: "claude".to_string(),
    ///     system_prompt: Some("You are a coding assistant.".to_string()),
    /// });
    /// // This rule now has highest priority (index 0)
    /// ```
    pub fn add_rule_at_top(&mut self, rule: RoutingRuleConfig) {
        self.rules.insert(0, rule);
        debug!(
            rules_count = self.rules.len(),
            "Added rule at top (highest priority)"
        );
    }

    /// Remove a routing rule by index
    ///
    /// # Arguments
    /// * `index` - Index of the rule to remove (0-based)
    ///
    /// # Returns
    /// * `Ok(())` - Rule removed successfully
    /// * `Err(AetherError::InvalidConfig)` - Index out of bounds
    ///
    /// # Example
    /// ```rust,no_run
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let mut config = Config::default();
    /// // Assuming rule exists at index 0
    /// config.remove_rule(0)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn remove_rule(&mut self, index: usize) -> Result<()> {
        if index < self.rules.len() {
            let removed = self.rules.remove(index);
            debug!(
                index = index,
                rule_type = %removed.get_rule_type(),
                regex = %removed.regex,
                rules_count = self.rules.len(),
                "Removed routing rule"
            );
            Ok(())
        } else {
            error!(
                index = index,
                max_index = self.rules.len().saturating_sub(1),
                "Rule index out of bounds"
            );
            Err(AetherError::invalid_config(format!(
                "Rule index {} out of bounds (valid range: 0-{})",
                index,
                self.rules.len().saturating_sub(1)
            )))
        }
    }

    /// Move a routing rule from one position to another
    ///
    /// This allows reordering rules to change their priority.
    ///
    /// # Arguments
    /// * `from` - Current index of the rule
    /// * `to` - Target index for the rule
    ///
    /// # Returns
    /// * `Ok(())` - Rule moved successfully
    /// * `Err(AetherError::InvalidConfig)` - Invalid indices
    ///
    /// # Example
    /// ```rust,no_run
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let mut config = Config::default();
    /// // Move rule from index 2 to index 0 (highest priority)
    /// config.move_rule(2, 0)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn move_rule(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.rules.len() {
            error!(
                from_index = from,
                max_index = self.rules.len().saturating_sub(1),
                "Source rule index out of bounds"
            );
            return Err(AetherError::invalid_config(format!(
                "Source index {} out of bounds (valid range: 0-{})",
                from,
                self.rules.len().saturating_sub(1)
            )));
        }
        if to >= self.rules.len() {
            error!(
                to_index = to,
                max_index = self.rules.len().saturating_sub(1),
                "Target rule index out of bounds"
            );
            return Err(AetherError::invalid_config(format!(
                "Target index {} out of bounds (valid range: 0-{})",
                to,
                self.rules.len().saturating_sub(1)
            )));
        }

        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
        debug!(from = from, to = to, "Moved routing rule");
        Ok(())
    }

    /// Get a routing rule by index
    ///
    /// # Arguments
    /// * `index` - Index of the rule to retrieve (0-based)
    ///
    /// # Returns
    /// * `Some(&RoutingRuleConfig)` - Reference to the rule if found
    /// * `None` - Index out of bounds
    ///
    /// # Example
    /// ```rust,no_run
    /// # use aethecore::config::Config;
    /// let config = Config::default();
    /// if let Some(rule) = config.get_rule(0) {
    ///     println!("First rule: {}", rule.regex);
    /// }
    /// ```
    pub fn get_rule(&self, index: usize) -> Option<&RoutingRuleConfig> {
        self.rules.get(index)
    }

    /// Get the number of routing rules
    ///
    /// # Returns
    /// * `usize` - Number of routing rules configured
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_hotkey, "Grave"); // Single ` key
        assert!(config.memory.enabled);
    }

    #[test]
    fn test_new_config() {
        let config = Config::new();
        assert_eq!(config.default_hotkey, "Grave"); // Single ` key
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("Command+Grave"));
        assert!(json.contains("memory"));
    }

    #[test]
    fn test_config_deserialization() {
        let json = r#"{"default_hotkey":"Grave"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.default_hotkey, "Grave");
        // memory field should use default
        assert_eq!(config.memory.embedding_model, "all-MiniLM-L6-v2");
    }

    #[test]
    fn test_memory_config_defaults() {
        let mem_config = MemoryConfig::default();
        assert!(mem_config.enabled);
        assert_eq!(mem_config.embedding_model, "all-MiniLM-L6-v2");
        assert_eq!(mem_config.max_context_items, 5);
        assert_eq!(mem_config.retention_days, 90);
        assert_eq!(mem_config.vector_db, "sqlite-vec");
        assert_eq!(mem_config.similarity_threshold, 0.7);
        assert!(!mem_config.excluded_apps.is_empty());
    }

    #[test]
    fn test_memory_config_serialization() {
        let mem_config = MemoryConfig::default();
        let json = serde_json::to_string(&mem_config).unwrap();
        assert!(json.contains("all-MiniLM-L6-v2"));
        assert!(json.contains("sqlite-vec"));
    }

    #[test]
    fn test_memory_config_deserialization() {
        let json = r#"{
            "enabled": false,
            "embedding_model": "custom-model",
            "max_context_items": 10,
            "retention_days": 30,
            "vector_db": "lancedb",
            "similarity_threshold": 0.8,
            "excluded_apps": ["com.example.app"]
        }"#;
        let config: MemoryConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.embedding_model, "custom-model");
        assert_eq!(config.max_context_items, 10);
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.vector_db, "lancedb");
        assert_eq!(config.similarity_threshold, 0.8);
        assert_eq!(config.excluded_apps, vec!["com.example.app"]);
    }

    #[test]
    fn test_default_excluded_apps() {
        let mem_config = MemoryConfig::default();
        assert!(mem_config
            .excluded_apps
            .contains(&"com.apple.keychainaccess".to_string()));
        assert!(mem_config
            .excluded_apps
            .contains(&"com.agilebits.onepassword7".to_string()));
    }

    #[test]
    fn test_config_validation_valid() {
        let mut config = Config::default();

        // Add a provider using test_config helper
        let provider = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider);
        config.general.default_provider = Some("openai".to_string());

        // Should pass validation
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_missing_default_provider() {
        let mut config = Config::default();
        config.general.default_provider = Some("nonexistent".to_string());

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_missing_api_key() {
        let mut config = Config::default();

        // Add OpenAI provider without API key
        let mut provider = ProviderConfig::test_config("gpt-4o");
        provider.api_key = None;
        config.providers.insert("openai".to_string(), provider);

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_temperature() {
        let mut config = Config::default();

        // Add provider with invalid temperature
        let mut provider = ProviderConfig::test_config("gpt-4o");
        provider.temperature = Some(3.0); // Invalid: > 2.0
        config.providers.insert("openai".to_string(), provider);

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_regex() {
        let mut config = Config::default();

        // Add valid provider using test_config helper
        let provider = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider);

        // Add command rule with invalid regex
        let mut invalid_rule = RoutingRuleConfig::command("[invalid(", "openai", None);
        invalid_rule.regex = "[invalid(".to_string();
        config.rules.push(invalid_rule);

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_rule_unknown_provider() {
        let mut config = Config::default();

        // Add command rule referencing unknown provider
        config
            .rules
            .push(RoutingRuleConfig::command(".*", "nonexistent", None));

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_load_from_toml() {
        let toml_str = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
api_key = "sk-test"
model = "gpt-4o"
color = "#10a37f"
timeout_seconds = 30
max_tokens = 4096
temperature = 0.7

[[rules]]
regex = "^/code"
provider = "openai"
system_prompt = "You are a coding assistant."

[memory]
enabled = true
max_context_items = 5
"##;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_hotkey, "Grave"); // Single ` key
        assert_eq!(config.general.default_provider, Some("openai".to_string()));
        assert!(config.providers.contains_key("openai"));
        assert_eq!(config.rules.len(), 1);
        assert!(config.memory.enabled);

        // Validation should pass
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_save_and_load() {
        use tempfile::NamedTempFile;

        let mut config = Config::default();

        // Add a provider using test_config helper
        let provider = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider);
        config.general.default_provider = Some("openai".to_string());

        // Save to temp file
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();
        config.save_to_file(path).unwrap();

        // Load back
        let loaded = Config::load_from_file(path).unwrap();
        assert_eq!(loaded.default_hotkey, config.default_hotkey);
        assert_eq!(
            loaded.general.default_provider,
            config.general.default_provider
        );
        assert!(loaded.providers.contains_key("openai"));
    }

    #[test]
    fn test_config_ollama_no_api_key() {
        let mut config = Config::default();

        // Ollama provider doesn't need API key
        let mut provider = ProviderConfig::test_config("llama3.2");
        provider.api_key = None; // Ollama doesn't need API key
        provider.provider_type = Some("ollama".to_string());
        config.providers.insert("ollama".to_string(), provider);

        // Should pass validation (no API key needed for Ollama)
        assert!(config.validate().is_ok());
    }

    // Additional comprehensive tests for Phase 6 - Task 8.1

    #[test]
    fn test_regex_validation_valid_patterns() {
        let mut config = Config::default();

        // Add valid provider using test_config helper
        let provider = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider);

        // Test various valid regex patterns
        let valid_patterns = vec![
            ".*",                // Match all
            "^/code",            // Start with /code
            "\\d+",              // One or more digits
            "hello|world",       // Alternatives
            "[a-zA-Z]+",         // Character class
            "^test$",            // Exact match
            "(foo|bar)\\s+\\w+", // Groups and word characters
        ];

        for pattern in valid_patterns {
            config.rules = vec![RoutingRuleConfig::command(pattern, "openai", None)];
            assert!(
                config.validate().is_ok(),
                "Pattern '{}' should be valid",
                pattern
            );
        }
    }

    #[test]
    fn test_regex_validation_invalid_patterns() {
        let mut config = Config::default();

        // Add valid provider using test_config helper
        let provider = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider);

        // Test various invalid regex patterns
        let invalid_patterns = vec![
            "[invalid(",   // Unclosed bracket
            "(unclosed",   // Unclosed parenthesis
            "**",          // Invalid quantifier
            "(?P<invalid", // Unclosed named group
            "[z-a]",       // Invalid range
        ];

        for pattern in invalid_patterns {
            let mut invalid_rule = RoutingRuleConfig::command(pattern, "openai", None);
            invalid_rule.regex = pattern.to_string(); // Ensure exact pattern is used
            config.rules = vec![invalid_rule];
            assert!(
                config.validate().is_err(),
                "Pattern '{}' should be invalid",
                pattern
            );
        }
    }

    #[test]
    fn test_shortcuts_config_defaults() {
        let shortcuts = ShortcutsConfig::default();
        assert_eq!(shortcuts.summon, "Command+Grave");
        assert_eq!(shortcuts.cancel, Some("Escape".to_string()));
    }

    #[test]
    fn test_shortcuts_config_serialization() {
        let shortcuts = ShortcutsConfig {
            summon: "Command+Shift+A".to_string(),
            cancel: Some("Escape".to_string()),
            command_prompt: "Command+Option+/".to_string(),
        };
        let json = serde_json::to_string(&shortcuts).unwrap();
        assert!(json.contains("Command+Shift+A"));
        assert!(json.contains("Escape"));
    }

    #[test]
    fn test_behavior_config_defaults() {
        let behavior = BehaviorConfig::default();
        assert_eq!(behavior.input_mode, "cut");
        assert_eq!(behavior.output_mode, "typewriter");
        assert_eq!(behavior.typing_speed, 50);
        assert!(!behavior.pii_scrubbing_enabled);
    }

    #[test]
    fn test_behavior_config_serialization() {
        let behavior = BehaviorConfig {
            input_mode: "copy".to_string(),
            output_mode: "instant".to_string(),
            typing_speed: 100,
            pii_scrubbing_enabled: true,
        };
        let json = serde_json::to_string(&behavior).unwrap();
        assert!(json.contains("copy"));
        assert!(json.contains("instant"));
        assert!(json.contains("100"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_atomic_write_creates_parent_directory() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir.path().join("nested").join("config.toml");

        let config = Config::default();
        config.save_to_file(&nested_path).unwrap();

        assert!(nested_path.exists());
    }

    #[test]
    fn test_atomic_write_overwrites_existing_file() {
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        // Write first config
        let mut config1 = Config::default();
        config1.default_hotkey = "Command+A".to_string();
        config1.save_to_file(path).unwrap();

        // Overwrite with second config
        let mut config2 = Config::default();
        config2.default_hotkey = "Command+B".to_string();
        config2.save_to_file(path).unwrap();

        // Load and verify
        let loaded = Config::load_from_file(path).unwrap();
        assert_eq!(loaded.default_hotkey, "Command+B");
    }

    #[test]
    fn test_config_validation_zero_timeout() {
        let mut config = Config::default();

        // Add provider with zero timeout
        let mut provider = ProviderConfig::test_config("gpt-4o");
        provider.timeout_seconds = 0; // Invalid: must be > 0
        config.providers.insert("openai".to_string(), provider);

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("timeout must be greater than 0"));
    }

    #[test]
    fn test_config_validation_memory_zero_max_context() {
        let mut config = Config::default();
        config.memory.max_context_items = 0;

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_context_items must be greater than 0"));
    }

    #[test]
    fn test_config_validation_memory_invalid_similarity() {
        let mut config = Config::default();
        config.memory.similarity_threshold = 1.5; // > 1.0

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("similarity_threshold must be between 0.0 and 1.0"));
    }

    #[test]
    fn test_provider_type_inference() {
        let mut provider = ProviderConfig::test_config("test-model");
        provider.provider_type = None; // Test inference

        // Test inference from provider name
        assert_eq!(provider.infer_provider_type("openai"), "openai");
        assert_eq!(provider.infer_provider_type("claude"), "claude");
        assert_eq!(provider.infer_provider_type("ollama"), "ollama");
        assert_eq!(provider.infer_provider_type("deepseek"), "openai"); // OpenAI-compatible
        assert_eq!(provider.infer_provider_type("custom"), "openai"); // Default
    }

    #[test]
    fn test_provider_type_explicit_override() {
        let mut provider = ProviderConfig::test_config("test-model");
        provider.provider_type = Some("custom".to_string());

        // Explicit type should override inference
        assert_eq!(provider.infer_provider_type("openai"), "custom");
    }

    #[test]
    fn test_full_config_conversion() {
        let mut config = Config::default();

        // Add providers using test_config helper
        let provider1 = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider1);

        let mut provider2 = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
        provider2.provider_type = Some("claude".to_string());
        config.providers.insert("claude".to_string(), provider2);

        // Convert to FullConfig
        let full_config: FullConfig = config.into();

        // Verify conversion
        assert_eq!(full_config.providers.len(), 2);
        assert!(full_config.providers.iter().any(|p| p.name == "openai"));
        assert!(full_config.providers.iter().any(|p| p.name == "claude"));
    }

    #[test]
    fn test_config_toml_round_trip() {
        let mut config = Config::default();

        // Add comprehensive configuration
        config.shortcuts = Some(ShortcutsConfig {
            summon: "Command+Shift+A".to_string(),
            cancel: Some("Escape".to_string()),
            command_prompt: "Command+Option+/".to_string(),
        });

        config.behavior = Some(BehaviorConfig {
            input_mode: "copy".to_string(),
            output_mode: "instant".to_string(),
            typing_speed: 100,
            pii_scrubbing_enabled: true,
        });

        let provider = ProviderConfig::test_config("gpt-4o");
        config.providers.insert("openai".to_string(), provider);
        config.general.default_provider = Some("openai".to_string());

        config.rules.push(RoutingRuleConfig::command(
            "^/code",
            "openai",
            Some("You are a coding assistant."),
        ));

        // Serialize to TOML
        let toml_str = toml::to_string_pretty(&config).unwrap();

        // Deserialize back
        let deserialized: Config = toml::from_str(&toml_str).unwrap();

        // Verify all fields
        assert_eq!(deserialized.default_hotkey, config.default_hotkey);
        assert_eq!(
            deserialized.shortcuts.as_ref().unwrap().summon,
            "Command+Shift+A"
        );
        assert_eq!(deserialized.behavior.as_ref().unwrap().input_mode, "copy");
        assert_eq!(deserialized.providers.len(), 1);
        // 4 builtin rules + 1 custom rule = 5 total
        assert_eq!(deserialized.rules.len(), 5);
        // Verify custom rule is present
        assert!(deserialized
            .rules
            .iter()
            .any(|r| r.regex.contains("code")));
        assert!(deserialized.validate().is_ok());
    }
}
