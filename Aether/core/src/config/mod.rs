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
    #[serde(default = "default_hotkey")]
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
    /// Skills configuration (Claude Agent Skills standard)
    #[serde(default)]
    pub skills: SkillsConfig,
    /// System Tools configuration (Tier 1: native Rust tools)
    #[serde(default)]
    pub tools: ToolsConfig,
    /// MCP (Model Context Protocol) configuration (Tier 2: external servers)
    #[serde(default)]
    pub mcp: McpConfig,
    /// Unified tools configuration (Phase 1 refactor: combines tools + mcp)
    /// If present, takes precedence over legacy [tools] and [mcp] sections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_tools: Option<UnifiedToolsConfig>,
    /// Trigger configuration (hotkey system refactor)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerConfig>,
    /// Smart conversation flow configuration
    #[serde(default)]
    pub smart_flow: SmartFlowConfig,
    /// Smart matching configuration (semantic detection system)
    #[serde(default)]
    pub smart_matching: SmartMatchingConfig,
    /// Dispatcher Layer configuration (intelligent tool routing)
    #[serde(default)]
    pub dispatcher: DispatcherConfigToml,
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

fn default_hotkey() -> String {
    "Grave".to_string()
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
    /// Enable multi-turn conversation by default
    /// When true, every conversation supports follow-up questions
    /// When false, use /chat command to start multi-turn mode
    #[serde(default)]
    pub multi_turn_enabled: bool,
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
            multi_turn_enabled: false,
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
    #[serde(default)]
    pub smart_matching: SmartMatchingConfig,
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
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
            smart_matching: config.smart_matching,
            skills: Some(config.skills),
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
    /// - "prepend" (default): Prepend system prompt to user message (for APIs that ignore system role)
    /// - "standard": Use a separate system message (for standard OpenAI-compatible APIs)
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

    // ========================================
    // Memory Compression Settings
    // ========================================

    /// Enable memory compression (LLM-based fact extraction)
    #[serde(default = "default_compression_enabled")]
    pub compression_enabled: bool,
    /// Idle timeout in seconds to trigger compression (default: 300 = 5 minutes)
    #[serde(default = "default_compression_idle_timeout")]
    pub compression_idle_timeout_seconds: u32,
    /// Number of conversation turns to trigger compression (default: 20)
    #[serde(default = "default_compression_turn_threshold")]
    pub compression_turn_threshold: u32,
    /// Background compression check interval in seconds (default: 3600 = 1 hour)
    #[serde(default = "default_compression_interval")]
    pub compression_interval_seconds: u32,
    /// Maximum memories to process per compression batch (default: 50)
    #[serde(default = "default_compression_batch_size")]
    pub compression_batch_size: u32,
    /// Similarity threshold for conflict detection (default: 0.85)
    #[serde(default = "default_conflict_similarity_threshold")]
    pub conflict_similarity_threshold: f32,
    /// Maximum facts to include in RAG context (default: 5)
    #[serde(default = "default_max_facts_in_context")]
    pub max_facts_in_context: u32,
    /// Maximum raw memories to fallback when facts insufficient (default: 3)
    #[serde(default = "default_raw_memory_fallback_count")]
    pub raw_memory_fallback_count: u32,
}

// Default value functions for MemoryConfig
fn default_enabled() -> bool {
    true
}

fn default_embedding_model() -> String {
    "bge-small-zh-v1.5".to_string()
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

// Compression configuration defaults
fn default_compression_enabled() -> bool {
    true
}

fn default_compression_idle_timeout() -> u32 {
    300 // 5 minutes
}

fn default_compression_turn_threshold() -> u32 {
    20
}

fn default_compression_interval() -> u32 {
    3600 // 1 hour
}

fn default_compression_batch_size() -> u32 {
    50
}

fn default_conflict_similarity_threshold() -> f32 {
    0.85
}

fn default_max_facts_in_context() -> u32 {
    5
}

fn default_raw_memory_fallback_count() -> u32 {
    3
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
            // Compression settings
            compression_enabled: default_compression_enabled(),
            compression_idle_timeout_seconds: default_compression_idle_timeout(),
            compression_turn_threshold: default_compression_turn_threshold(),
            compression_interval_seconds: default_compression_interval(),
            compression_batch_size: default_compression_batch_size(),
            conflict_similarity_threshold: default_conflict_similarity_threshold(),
            max_facts_in_context: default_max_facts_in_context(),
            raw_memory_fallback_count: default_raw_memory_fallback_count(),
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
/// Controls AI-powered intent detection and capability invocation.
/// AI analyzes user input and decides whether to invoke capabilities (search, video, etc.)
/// or respond directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentDetectionConfig {
    /// Enable intent detection globally
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Use AI for intent detection (supports all languages)
    #[serde(default = "default_true")]
    pub use_ai: bool,

    /// Confidence threshold for AI detection (0.0 - 1.0)
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,

    /// Timeout for AI detection in milliseconds
    #[serde(default = "default_ai_timeout_ms")]
    pub ai_timeout_ms: u64,

    // Capability enables
    /// Enable search capability (weather, news, general queries)
    #[serde(default = "default_true")]
    pub search: bool,

    /// Enable video capability (YouTube, Bilibili analysis)
    #[serde(default = "default_true")]
    pub video: bool,

    /// Enable skill capability (future)
    #[serde(default = "default_false")]
    pub skill: bool,

    /// Enable MCP capability (future)
    #[serde(default = "default_false")]
    pub mcp: bool,
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
            use_ai: true,
            confidence_threshold: default_confidence_threshold(),
            ai_timeout_ms: default_ai_timeout_ms(),
            search: true,
            video: true,
            skill: false,
            mcp: false,
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

// ============================================================================
// Smart Matching Configuration (Semantic Detection System)
// ============================================================================

/// Smart matching configuration for the semantic detection system
///
/// Controls multi-layer semantic matching with configurable thresholds:
/// - Layer 1: Fast path (command/regex) - highest confidence
/// - Layer 2: Keyword matching - weighted scoring
/// - Layer 3: Context-aware inference - multi-turn, app, time
/// - Layer 4: AI detection fallback - AI-first approach
///
/// # Example TOML
///
/// ```toml
/// [smart_matching]
/// enabled = true
/// command_confidence = 1.0
/// regex_threshold = 0.9
/// keyword_threshold = 0.7
/// ai_threshold = 0.6
/// enable_context_inference = true
///
/// [[smart_matching.context_rules]]
/// id = "weather_followup"
/// condition_type = "pending_param"
/// param_name = "location"
/// intent = "search"
/// action_type = "complete_param"
/// use_input_as_value = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartMatchingConfig {
    /// Enable/disable smart matching system
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Confidence threshold for command rules (exact match = 1.0)
    #[serde(default = "default_command_confidence")]
    pub command_confidence: f64,

    /// Confidence threshold for regex pattern matching (0.0 - 1.0)
    #[serde(default = "default_regex_threshold")]
    pub regex_threshold: f64,

    /// Confidence threshold for keyword matching (0.0 - 1.0)
    #[serde(default = "default_keyword_threshold")]
    pub keyword_threshold: f64,

    /// Confidence threshold below which AI detection is triggered (0.0 - 1.0)
    #[serde(default = "default_ai_threshold")]
    pub ai_threshold: f64,

    /// Enable context-aware inference (multi-turn, app context, time context)
    #[serde(default = "default_true")]
    pub enable_context_inference: bool,

    /// Context rules for special matching behavior
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_rules: Vec<ContextRuleConfig>,

    /// Keyword rules for weighted matching (alternative to regex-based rules)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyword_rules: Vec<KeywordRuleConfig>,
}

fn default_command_confidence() -> f64 {
    1.0
}

fn default_regex_threshold() -> f64 {
    0.9
}

fn default_keyword_threshold() -> f64 {
    0.7
}

fn default_ai_threshold() -> f64 {
    0.6
}

impl Default for SmartMatchingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command_confidence: default_command_confidence(),
            regex_threshold: default_regex_threshold(),
            keyword_threshold: default_keyword_threshold(),
            ai_threshold: default_ai_threshold(),
            enable_context_inference: true,
            context_rules: Vec::new(),
            keyword_rules: Vec::new(),
        }
    }
}

/// Context rule configuration for special matching behavior
///
/// Defines conditions and actions for context-aware matching.
/// Used for multi-turn conversation, app-specific behavior, and time-based rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRuleConfig {
    /// Unique rule identifier
    pub id: String,

    /// Condition type: "pending_param", "app_context", "time_context", "conversation"
    #[serde(default = "default_condition_type")]
    pub condition_type: String,

    /// Parameter name (for pending_param condition)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param_name: Option<String>,

    /// Intent type to match (for pending_param condition)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,

    /// Bundle IDs to match (for app_context condition)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bundle_ids: Vec<String>,

    /// Hours of day to match (for time_context condition, 0-23)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hours: Vec<u8>,

    /// Days of week to match (for time_context condition, 0=Sunday)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days_of_week: Vec<u8>,

    /// Action type: "complete_param", "add_capability", "set_provider", "add_prompt"
    #[serde(default = "default_action_type")]
    pub action_type: String,

    /// Use input as parameter value (for complete_param action)
    #[serde(default)]
    pub use_input_as_value: bool,

    /// Capability to add (for add_capability action)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,

    /// Provider name (for set_provider action)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// System prompt to add (for add_prompt action)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

fn default_condition_type() -> String {
    "pending_param".to_string()
}

fn default_action_type() -> String {
    "complete_param".to_string()
}

impl Default for ContextRuleConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            condition_type: default_condition_type(),
            param_name: None,
            intent: None,
            bundle_ids: Vec::new(),
            hours: Vec::new(),
            days_of_week: Vec::new(),
            action_type: default_action_type(),
            use_input_as_value: false,
            capability: None,
            provider: None,
            system_prompt: None,
        }
    }
}

/// Keyword rule configuration for weighted keyword matching
///
/// Alternative to regex-based rules, uses weighted scoring for intent detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordRuleConfig {
    /// Unique rule identifier
    pub id: String,

    /// Rule name (for display)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Keywords with optional weights: "keyword" or "keyword:1.5"
    pub keywords: Vec<String>,

    /// Match mode: "any" (OR), "all" (AND), "weighted" (sum/total)
    #[serde(default = "default_match_mode")]
    pub match_mode: String,

    /// Intent type for matched rules
    pub intent_type: String,

    /// System prompt when this rule matches
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Capabilities to enable
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,

    /// Minimum score threshold (0.0 - 1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_score: Option<f32>,
}

fn default_match_mode() -> String {
    "any".to_string()
}

impl Default for KeywordRuleConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: None,
            keywords: Vec::new(),
            match_mode: default_match_mode(),
            intent_type: "general".to_string(),
            system_prompt: None,
            capabilities: Vec::new(),
            min_score: None,
        }
    }
}

impl KeywordRuleConfig {
    /// Parse keywords with optional weights
    ///
    /// Format: "keyword" (weight=1.0) or "keyword:1.5"
    pub fn parse_keywords(&self) -> Vec<(String, f32)> {
        self.keywords
            .iter()
            .map(|s| {
                if let Some((keyword, weight_str)) = s.rsplit_once(':') {
                    let weight = weight_str.parse::<f32>().unwrap_or(1.0);
                    (keyword.to_string(), weight)
                } else {
                    (s.clone(), 1.0)
                }
            })
            .collect()
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

/// MCP (Model Context Protocol) configuration
///
/// Controls external MCP server connections (Tier 2 Extensions)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// Enable MCP capability
    #[serde(default = "default_mcp_enabled")]
    pub enabled: bool,

    /// External servers configuration
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_servers: Vec<McpExternalServerConfig>,
}

/// Configuration for System Tools (Tier 1: native Rust tools)
///
/// System Tools are always available and run as native Rust code.
/// They provide file system, git, shell, and system info capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// Enable filesystem service
    #[serde(default = "default_true")]
    pub fs_enabled: bool,

    /// Allowed filesystem roots (paths the fs service can access)
    #[serde(default)]
    pub allowed_roots: Vec<String>,

    /// Enable git service
    #[serde(default = "default_true")]
    pub git_enabled: bool,

    /// Allowed git repositories (paths the git service can access)
    #[serde(default)]
    pub allowed_repos: Vec<String>,

    /// Enable shell service
    #[serde(default)]
    pub shell_enabled: bool,

    /// Allowed shell commands (whitelist for security)
    #[serde(default)]
    pub allowed_commands: Vec<String>,

    /// Shell command timeout in seconds
    #[serde(default = "default_shell_timeout")]
    pub shell_timeout_seconds: u64,

    /// Enable system info service
    #[serde(default = "default_true")]
    pub system_info_enabled: bool,
}

/// Configuration for external MCP servers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpExternalServerConfig {
    /// Server name (unique identifier)
    pub name: String,

    /// Command to execute
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,

    /// Required runtime (node, python, bun, deno)
    #[serde(default)]
    pub requires_runtime: Option<String>,

    /// Request timeout in seconds
    #[serde(default = "default_mcp_timeout")]
    pub timeout_seconds: u64,
}

fn default_mcp_enabled() -> bool {
    true
}

fn default_shell_timeout() -> u64 {
    30
}

fn default_mcp_timeout() -> u64 {
    30
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: default_mcp_enabled(),
            external_servers: Vec::new(),
        }
    }
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            fs_enabled: true,
            allowed_roots: Vec::new(), // Empty means current directory only
            git_enabled: true,
            allowed_repos: Vec::new(), // Empty means current directory only
            shell_enabled: false, // Disabled by default for security
            allowed_commands: vec![
                "ls".to_string(),
                "cat".to_string(),
                "echo".to_string(),
                "pwd".to_string(),
            ],
            shell_timeout_seconds: default_shell_timeout(),
            system_info_enabled: true,
        }
    }
}

// =============================================================================
// Unified Tools Configuration (Phase 1 Refactor: Low-coupling Architecture)
// =============================================================================
//
// This unified configuration structure combines:
// - System Tools (Tier 1: native Rust tools)
// - MCP External Servers (Tier 2: external process tools)
//
// Benefits:
// - Single source of truth for all tools configuration
// - Cleaner TOML structure with nested tables
// - Easier to extend with new tool types
// - Better configuration validation
//
// Migration path:
// - Old [tools] + [mcp] sections are still supported for backward compatibility
// - New [unified_tools] section takes precedence if present
// - Config::get_effective_tools_config() merges both formats

/// Unified tools configuration (combines System Tools + MCP External Servers)
///
/// New TOML format:
/// ```toml
/// [unified_tools]
/// enabled = true
///
/// [unified_tools.native.fs]
/// enabled = true
/// allowed_roots = ["~", "/tmp"]
///
/// [unified_tools.native.git]
/// enabled = true
/// allowed_repos = ["~/projects"]
///
/// [unified_tools.native.shell]
/// enabled = false
/// timeout_seconds = 30
/// allowed_commands = ["ls", "cat"]
///
/// [unified_tools.native.system_info]
/// enabled = true
///
/// [unified_tools.mcp.github]
/// command = "node"
/// args = ["~/.mcp/github/index.js"]
/// requires_runtime = "node"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedToolsConfig {
    /// Master switch for all tools (both native and MCP)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Native system tools configuration
    #[serde(default)]
    pub native: NativeToolsConfig,

    /// MCP external servers configuration (keyed by server name)
    #[serde(default)]
    pub mcp: HashMap<String, McpServerConfig>,
}

/// Configuration for native system tools (Tier 1)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NativeToolsConfig {
    /// Filesystem service configuration
    #[serde(default)]
    pub fs: Option<FsToolConfig>,

    /// Git service configuration
    #[serde(default)]
    pub git: Option<GitToolConfig>,

    /// Shell service configuration
    #[serde(default)]
    pub shell: Option<ShellToolConfig>,

    /// System info service configuration
    #[serde(default)]
    pub system_info: Option<SystemInfoToolConfig>,

    /// Clipboard read service configuration
    #[serde(default)]
    pub clipboard: Option<ClipboardToolConfig>,

    /// Screen capture service configuration
    #[serde(default)]
    pub screen_capture: Option<ScreenCaptureToolConfig>,

    /// Search tool service configuration
    #[serde(default)]
    pub search: Option<SearchToolConfig>,
}

/// Filesystem tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsToolConfig {
    /// Enable filesystem service
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allowed filesystem roots (paths the fs service can access)
    /// Empty means current directory only
    #[serde(default)]
    pub allowed_roots: Vec<String>,
}

impl Default for FsToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_roots: Vec::new(),
        }
    }
}

/// Git tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitToolConfig {
    /// Enable git service
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allowed git repositories (paths the git service can access)
    /// Empty means current directory only
    #[serde(default)]
    pub allowed_repos: Vec<String>,
}

impl Default for GitToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_repos: Vec::new(),
        }
    }
}

/// Shell tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellToolConfig {
    /// Enable shell service (disabled by default for security)
    #[serde(default)]
    pub enabled: bool,

    /// Shell command timeout in seconds
    #[serde(default = "default_shell_timeout")]
    pub timeout_seconds: u64,

    /// Allowed shell commands (whitelist for security)
    #[serde(default = "default_shell_commands")]
    pub allowed_commands: Vec<String>,
}

fn default_shell_commands() -> Vec<String> {
    vec![
        "ls".to_string(),
        "cat".to_string(),
        "echo".to_string(),
        "pwd".to_string(),
    ]
}

impl Default for ShellToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_seconds: default_shell_timeout(),
            allowed_commands: default_shell_commands(),
        }
    }
}

/// System info tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoToolConfig {
    /// Enable system info service
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for SystemInfoToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Clipboard tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardToolConfig {
    /// Enable clipboard read service
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for ClipboardToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Screen capture tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenCaptureToolConfig {
    /// Enable screen capture service
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum image dimension (width or height)
    #[serde(default = "default_max_dimension")]
    pub max_dimension: u32,

    /// JPEG quality for captured images (0-100)
    #[serde(default = "default_jpeg_quality")]
    pub jpeg_quality: u8,
}

fn default_max_dimension() -> u32 {
    1920
}

fn default_jpeg_quality() -> u8 {
    85
}

impl Default for ScreenCaptureToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_dimension: default_max_dimension(),
            jpeg_quality: default_jpeg_quality(),
        }
    }
}

/// Search tool configuration (wraps existing SearchRegistry as tool)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchToolConfig {
    /// Enable search tool
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default maximum number of search results
    #[serde(default = "default_search_tool_max_results")]
    pub default_max_results: usize,

    /// Default search timeout in seconds
    #[serde(default = "default_search_tool_timeout_seconds")]
    pub default_timeout_seconds: u64,
}

fn default_search_tool_max_results() -> usize {
    5
}

fn default_search_tool_timeout_seconds() -> u64 {
    10
}

impl Default for SearchToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_max_results: default_search_tool_max_results(),
            default_timeout_seconds: default_search_tool_timeout_seconds(),
        }
    }
}

/// MCP external server configuration (unified format)
///
/// This is similar to McpExternalServerConfig but with a cleaner structure
/// where the server name is the TOML table key instead of a field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to execute
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,

    /// Required runtime (node, python, bun, deno)
    #[serde(default)]
    pub requires_runtime: Option<String>,

    /// Request timeout in seconds
    #[serde(default = "default_mcp_timeout")]
    pub timeout_seconds: u64,

    /// Enable this server (allows disabling without removing config)
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for UnifiedToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            native: NativeToolsConfig::default(),
            mcp: HashMap::new(),
        }
    }
}

impl UnifiedToolsConfig {
    /// Create from legacy ToolsConfig and McpConfig (migration helper)
    pub fn from_legacy(tools: &ToolsConfig, mcp: &McpConfig) -> Self {
        let mut unified = Self {
            enabled: mcp.enabled,
            native: NativeToolsConfig {
                fs: Some(FsToolConfig {
                    enabled: tools.fs_enabled,
                    allowed_roots: tools.allowed_roots.clone(),
                }),
                git: Some(GitToolConfig {
                    enabled: tools.git_enabled,
                    allowed_repos: tools.allowed_repos.clone(),
                }),
                shell: Some(ShellToolConfig {
                    enabled: tools.shell_enabled,
                    timeout_seconds: tools.shell_timeout_seconds,
                    allowed_commands: tools.allowed_commands.clone(),
                }),
                system_info: Some(SystemInfoToolConfig {
                    enabled: tools.system_info_enabled,
                }),
                // New tools use defaults (not in legacy config)
                clipboard: None,
                screen_capture: None,
                search: None,
            },
            mcp: HashMap::new(),
        };

        // Convert external servers to new format
        for server in &mcp.external_servers {
            unified.mcp.insert(
                server.name.clone(),
                McpServerConfig {
                    command: server.command.clone(),
                    args: server.args.clone(),
                    env: server.env.clone(),
                    cwd: server.cwd.clone(),
                    requires_runtime: server.requires_runtime.clone(),
                    timeout_seconds: server.timeout_seconds,
                    enabled: true,
                },
            );
        }

        unified
    }

    /// Check if filesystem service is enabled
    pub fn is_fs_enabled(&self) -> bool {
        self.enabled && self.native.fs.as_ref().map_or(true, |c| c.enabled)
    }

    /// Check if git service is enabled
    pub fn is_git_enabled(&self) -> bool {
        self.enabled && self.native.git.as_ref().map_or(true, |c| c.enabled)
    }

    /// Check if shell service is enabled
    pub fn is_shell_enabled(&self) -> bool {
        self.enabled && self.native.shell.as_ref().map_or(false, |c| c.enabled)
    }

    /// Check if system info service is enabled
    pub fn is_system_info_enabled(&self) -> bool {
        self.enabled && self.native.system_info.as_ref().map_or(true, |c| c.enabled)
    }

    /// Get filesystem allowed roots
    pub fn fs_allowed_roots(&self) -> Vec<String> {
        self.native
            .fs
            .as_ref()
            .map_or(Vec::new(), |c| c.allowed_roots.clone())
    }

    /// Get git allowed repos
    pub fn git_allowed_repos(&self) -> Vec<String> {
        self.native
            .git
            .as_ref()
            .map_or(Vec::new(), |c| c.allowed_repos.clone())
    }

    /// Get shell configuration
    pub fn shell_config(&self) -> ShellToolConfig {
        self.native.shell.clone().unwrap_or_default()
    }

    /// Check if clipboard service is enabled
    pub fn is_clipboard_enabled(&self) -> bool {
        self.enabled && self.native.clipboard.as_ref().map_or(true, |c| c.enabled)
    }

    /// Check if screen capture service is enabled
    pub fn is_screen_capture_enabled(&self) -> bool {
        self.enabled && self.native.screen_capture.as_ref().map_or(true, |c| c.enabled)
    }

    /// Get screen capture configuration
    pub fn screen_capture_config(&self) -> ScreenCaptureToolConfig {
        self.native.screen_capture.clone().unwrap_or_default()
    }

    /// Check if search tool service is enabled
    pub fn is_search_tool_enabled(&self) -> bool {
        self.enabled && self.native.search.as_ref().map_or(true, |c| c.enabled)
    }

    /// Get search tool configuration
    pub fn search_tool_config(&self) -> SearchToolConfig {
        self.native.search.clone().unwrap_or_default()
    }

    /// Get all enabled MCP servers
    pub fn enabled_mcp_servers(&self) -> Vec<(&String, &McpServerConfig)> {
        self.mcp
            .iter()
            .filter(|(_, config)| config.enabled)
            .collect()
    }
}

/// Skills configuration (Claude Agent Skills standard)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Enable skills capability
    #[serde(default = "default_skills_enabled")]
    pub enabled: bool,

    /// Skills directory path (relative to config dir or absolute)
    #[serde(default = "default_skills_dir")]
    pub skills_dir: String,

    /// Enable auto-matching skills based on user input
    #[serde(default = "default_auto_match_enabled")]
    pub auto_match_enabled: bool,
}

fn default_skills_enabled() -> bool {
    true
}

fn default_skills_dir() -> String {
    "skills".to_string()
}

fn default_auto_match_enabled() -> bool {
    false // Off by default, explicit /skill command required
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: default_skills_enabled(),
            skills_dir: default_skills_dir(),
            auto_match_enabled: default_auto_match_enabled(),
        }
    }
}

impl SkillsConfig {
    /// Get the full path to the skills directory
    ///
    /// If skills_dir is relative, it's relative to ~/.config/aether/
    /// If absolute, use as-is
    pub fn get_skills_dir_path(&self) -> std::path::PathBuf {
        let path = std::path::Path::new(&self.skills_dir);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative to config directory
            if let Some(home) = dirs::home_dir() {
                home.join(".config").join("aether").join(&self.skills_dir)
            } else {
                path.to_path_buf()
            }
        }
    }
}

/// Configuration for the Dispatcher Layer (Aether Cortex)
///
/// The Dispatcher Layer provides intelligent tool routing through three layers:
/// - L1: Regex-based pattern matching (highest confidence)
/// - L2: Semantic keyword matching (medium confidence)
/// - L3: AI-powered inference (variable confidence)
///
/// When a tool match has low confidence, the system can show a confirmation
/// dialog to the user before execution.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher]
/// enabled = true
/// l3_enabled = true
/// l3_timeout_ms = 5000
/// confirmation_threshold = 0.7
/// confirmation_timeout_ms = 30000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherConfigToml {
    /// Whether the dispatcher is enabled (default: true)
    #[serde(default = "default_dispatcher_enabled")]
    pub enabled: bool,

    /// Whether L3 AI inference is enabled (default: true)
    #[serde(default = "default_dispatcher_l3_enabled")]
    pub l3_enabled: bool,

    /// L3 routing timeout in milliseconds (default: 5000)
    #[serde(default = "default_dispatcher_l3_timeout")]
    pub l3_timeout_ms: u64,

    /// Confidence threshold below which confirmation is required (0.0-1.0, default: 0.7)
    /// - Values >= 1.0 disable confirmation entirely
    /// - Values <= 0.0 always require confirmation
    #[serde(default = "default_dispatcher_confirmation_threshold")]
    pub confirmation_threshold: f32,

    /// Confirmation dialog timeout in milliseconds (default: 30000)
    #[serde(default = "default_dispatcher_confirmation_timeout")]
    pub confirmation_timeout_ms: u64,

    /// Whether confirmation dialogs are enabled (default: true)
    #[serde(default = "default_dispatcher_confirmation_enabled")]
    pub confirmation_enabled: bool,
}

fn default_dispatcher_enabled() -> bool {
    true
}

fn default_dispatcher_l3_enabled() -> bool {
    true
}

fn default_dispatcher_l3_timeout() -> u64 {
    5000 // 5 seconds
}

fn default_dispatcher_confirmation_threshold() -> f32 {
    0.7 // Require confirmation if confidence < 70%
}

fn default_dispatcher_confirmation_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_dispatcher_confirmation_enabled() -> bool {
    true
}

impl Default for DispatcherConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_dispatcher_enabled(),
            l3_enabled: default_dispatcher_l3_enabled(),
            l3_timeout_ms: default_dispatcher_l3_timeout(),
            confirmation_threshold: default_dispatcher_confirmation_threshold(),
            confirmation_timeout_ms: default_dispatcher_confirmation_timeout(),
            confirmation_enabled: default_dispatcher_confirmation_enabled(),
        }
    }
}

impl DispatcherConfigToml {
    /// Validate the configuration values
    ///
    /// # Returns
    /// * `Ok(())` - Configuration is valid
    /// * `Err(String)` - Validation error message
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Validate confirmation threshold range
        if self.confirmation_threshold < 0.0 {
            return Err(format!(
                "confirmation_threshold must be >= 0.0, got {}",
                self.confirmation_threshold
            ));
        }
        if self.confirmation_threshold > 1.0 {
            warn!(
                threshold = self.confirmation_threshold,
                "confirmation_threshold > 1.0 will disable confirmation entirely"
            );
        }

        // Validate L3 timeout
        if self.l3_timeout_ms == 0 {
            return Err("l3_timeout_ms must be > 0".to_string());
        }
        if self.l3_timeout_ms > 60000 {
            warn!(
                timeout = self.l3_timeout_ms,
                "l3_timeout_ms > 60000ms may cause poor user experience"
            );
        }

        // Validate confirmation timeout
        if self.confirmation_timeout_ms == 0 {
            return Err("confirmation_timeout_ms must be > 0".to_string());
        }

        Ok(())
    }

    /// Convert to internal DispatcherConfig
    pub fn to_dispatcher_config(&self) -> crate::dispatcher::DispatcherConfig {
        use crate::dispatcher::{ConfirmationConfig, DispatcherConfig};

        DispatcherConfig {
            enabled: self.enabled,
            l3_enabled: self.l3_enabled,
            l3_timeout_ms: self.l3_timeout_ms,
            l3_confidence_threshold: self.confirmation_threshold,
            confirmation: ConfirmationConfig {
                enabled: self.confirmation_enabled,
                threshold: self.confirmation_threshold,
                timeout_ms: self.confirmation_timeout_ms,
                show_parameters: true,
                skip_native_tools: false,
            },
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
            // Use builtin_rules() to ensure consistency with merge_builtin_rules()
            rules: Config::builtin_rules(),
            shortcuts: Some(ShortcutsConfig::default()),
            behavior: Some(BehaviorConfig::default()),
            search: None,
            video: Some(VideoConfig::default()),
            skills: SkillsConfig::default(),
            tools: ToolsConfig::default(),
            mcp: McpConfig::default(),
            unified_tools: None, // Use legacy tools + mcp by default for backward compatibility
            trigger: Some(TriggerConfig::default()),
            smart_flow: SmartFlowConfig::default(),
            smart_matching: SmartMatchingConfig::default(),
            dispatcher: DispatcherConfigToml::default(),
        }
    }
}

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Get effective tools configuration (unified format)
    ///
    /// This method provides a unified view of tools configuration:
    /// - If `unified_tools` is present, it takes precedence
    /// - Otherwise, creates unified config from legacy `tools` + `mcp` sections
    ///
    /// This enables gradual migration from legacy config format to unified format.
    pub fn get_effective_tools_config(&self) -> UnifiedToolsConfig {
        if let Some(unified) = &self.unified_tools {
            unified.clone()
        } else {
            UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp)
        }
    }

    /// Check if using new unified tools configuration
    pub fn is_using_unified_tools(&self) -> bool {
        self.unified_tools.is_some()
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

        // Pre-process TOML: Migrate [mcp.builtin] to [tools] if needed
        let contents = Self::migrate_mcp_builtin_in_toml(&contents)?;

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
    /// Returns the preset routing rules for builtin commands (/search, /mcp, /skill, /video, /chat).
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
            // /skill command - Claude Agent Skills instruction injection
            // Usage: /skill <skill_name> <user_input>
            // The skill_name is extracted and used to look up instructions from the skills registry
            RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: r"^/skill\s+".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("You are a helpful AI assistant. Follow the skill instructions provided in the context to complete the task.".to_string()),
                strip_prefix: Some(true),
                capabilities: Some(vec!["skills".to_string(), "memory".to_string()]),
                intent_type: Some("skills".to_string()),
                context_format: Some("markdown".to_string()),
                skill_id: None, // Extracted dynamically from command argument
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
                provider: Some("openai".to_string()),
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
            // /chat command - General conversation assistant
            RoutingRuleConfig {
                rule_type: Some("command".to_string()),
                is_builtin: true,
                regex: r"^/chat\s+".to_string(),
                provider: Some("openai".to_string()),
                system_prompt: Some("You are a helpful AI assistant. Engage in natural conversation and provide helpful responses.".to_string()),
                strip_prefix: Some(true),
                capabilities: Some(vec!["memory".to_string()]),
                intent_type: Some("general_chat".to_string()),
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
    /// Builtin rules (/search, /mcp, /skill, /video, /chat) are prepended to user rules
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

    /// Migrate [mcp.builtin] to [tools] in raw TOML
    ///
    /// This is a pre-parsing migration that handles the rename-builtin-to-system-tools
    /// proposal. If the old [mcp.builtin] section exists but [tools] doesn't,
    /// the old section is copied to [tools].
    ///
    /// # Arguments
    /// * `contents` - Raw TOML string
    ///
    /// # Returns
    /// * Modified TOML string with migration applied
    fn migrate_mcp_builtin_in_toml(contents: &str) -> Result<String> {
        // Parse as raw TOML value
        let mut value: toml::Value = toml::from_str(contents).map_err(|e| {
            AetherError::invalid_config(format!("Failed to parse TOML for migration: {}", e))
        })?;

        // Check if migration is needed
        let needs_migration = {
            let has_mcp_builtin = value
                .get("mcp")
                .and_then(|mcp| mcp.get("builtin"))
                .is_some();
            let has_tools = value.get("tools").is_some();

            has_mcp_builtin && !has_tools
        };

        if !needs_migration {
            return Ok(contents.to_string());
        }

        // Perform migration
        warn!("Migrating deprecated [mcp.builtin] section to [tools]");

        // Extract mcp.builtin
        let builtin = value
            .get("mcp")
            .and_then(|mcp| mcp.get("builtin"))
            .cloned();

        if let Some(builtin_value) = builtin {
            // Add as [tools]
            if let toml::Value::Table(ref mut table) = value {
                table.insert("tools".to_string(), builtin_value);

                // Remove [mcp.builtin]
                if let Some(toml::Value::Table(ref mut mcp)) = table.get_mut("mcp") {
                    mcp.remove("builtin");
                }
            }

            info!("Successfully migrated [mcp.builtin] to [tools]");
        }

        // Serialize back to TOML
        toml::to_string_pretty(&value).map_err(|e| {
            AetherError::invalid_config(format!("Failed to serialize migrated TOML: {}", e))
        })
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
mod tests;
