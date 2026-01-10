//! Smart flow configuration types
//!
//! Contains intent detection and semantic matching configuration:
//! - SmartFlowConfig: Master config for smart conversation flow
//! - IntentDetectionConfig: AI-powered intent detection settings
//! - SuggestionParsingConfig: AI response suggestion parsing
//! - SmartMatchingConfig: Multi-layer semantic matching system
//! - ContextRuleConfig: Context-aware matching rules
//! - KeywordRuleConfig: Weighted keyword matching rules

use serde::{Deserialize, Serialize};

use super::search::{default_false, default_true};

// =============================================================================
// SmartFlowConfig
// =============================================================================

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

// =============================================================================
// IntentDetectionConfig
// =============================================================================

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

pub fn default_confidence_threshold() -> f64 {
    0.7
}

pub fn default_ai_timeout_ms() -> u64 {
    3000
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

// =============================================================================
// SuggestionParsingConfig
// =============================================================================

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

pub fn default_max_suggestions() -> usize {
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

// =============================================================================
// SmartMatchingConfig
// =============================================================================

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

pub fn default_command_confidence() -> f64 {
    1.0
}

pub fn default_regex_threshold() -> f64 {
    0.9
}

pub fn default_keyword_threshold() -> f64 {
    0.7
}

pub fn default_ai_threshold() -> f64 {
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

// =============================================================================
// ContextRuleConfig
// =============================================================================

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

pub fn default_condition_type() -> String {
    "pending_param".to_string()
}

pub fn default_action_type() -> String {
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

// =============================================================================
// KeywordRuleConfig
// =============================================================================

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

pub fn default_match_mode() -> String {
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
