//! Thinking Levels System for LLM Reasoning Depth Control
//!
//! This module provides a comprehensive thinking levels system inspired by Moltbot,
//! supporting 6 levels of reasoning depth with provider-specific adaptations.
//!
//! # Overview
//!
//! The thinking levels system allows controlling how much "thinking" or reasoning
//! an LLM performs before generating a response. Higher thinking levels generally
//! produce more thorough, well-reasoned responses at the cost of latency and tokens.
//!
//! # Levels
//!
//! - `Off`: No extended thinking, fastest response
//! - `Minimal`: Brief internal reasoning (default)
//! - `Low`: Basic thinking process
//! - `Medium`: Balanced thinking depth
//! - `High`: Detailed reasoning and analysis
//! - `XHigh`: Deep extended thinking (model-specific)
//!
//! # Example
//!
//! ```rust
//! use alephcore::agents::thinking::{ThinkLevel, normalize_think_level};
//!
//! // Parse user input
//! let level = normalize_think_level("ultrathink").unwrap();
//! assert_eq!(level, ThinkLevel::High);
//!
//! // Check model support
//! use alephcore::agents::thinking::supports_xhigh_thinking;
//! let supports = supports_xhigh_thinking("claude", "claude-opus-4-5-20251101");
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// =============================================================================
// ThinkLevel Enum
// =============================================================================

/// Thinking level for LLM reasoning depth control
///
/// Provides 6 levels of reasoning depth from no thinking to extended deep reasoning.
/// Inspired by Moltbot's ThinkLevel system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkLevel {
    /// No extended thinking, fastest response
    Off,
    /// Minimal thinking, brief internal reasoning (default)
    #[default]
    Minimal,
    /// Low thinking depth
    Low,
    /// Medium thinking depth (balanced)
    Medium,
    /// High thinking depth, detailed reasoning
    High,
    /// Extended high thinking (xhigh), deepest reasoning
    /// Only supported by specific models (e.g., GPT-5.2, Claude Opus)
    XHigh,
}

impl ThinkLevel {
    /// All available thinking levels in order (lowest to highest)
    pub const ALL: &'static [ThinkLevel] = &[
        ThinkLevel::Off,
        ThinkLevel::Minimal,
        ThinkLevel::Low,
        ThinkLevel::Medium,
        ThinkLevel::High,
        ThinkLevel::XHigh,
    ];

    /// Get the next lower thinking level for fallback
    ///
    /// Returns `None` if already at the lowest level (Off).
    pub fn fallback(&self) -> Option<ThinkLevel> {
        match self {
            ThinkLevel::XHigh => Some(ThinkLevel::High),
            ThinkLevel::High => Some(ThinkLevel::Medium),
            ThinkLevel::Medium => Some(ThinkLevel::Low),
            ThinkLevel::Low => Some(ThinkLevel::Minimal),
            ThinkLevel::Minimal => Some(ThinkLevel::Off),
            ThinkLevel::Off => None,
        }
    }

    /// Get numeric weight for comparison (higher = more thinking)
    pub fn weight(&self) -> u8 {
        match self {
            ThinkLevel::Off => 0,
            ThinkLevel::Minimal => 1,
            ThinkLevel::Low => 2,
            ThinkLevel::Medium => 3,
            ThinkLevel::High => 4,
            ThinkLevel::XHigh => 5,
        }
    }

    /// Check if this level is higher than another
    pub fn is_higher_than(&self, other: &ThinkLevel) -> bool {
        self.weight() > other.weight()
    }

    /// Check if this level is lower than another
    pub fn is_lower_than(&self, other: &ThinkLevel) -> bool {
        self.weight() < other.weight()
    }

    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            ThinkLevel::Off => "Off",
            ThinkLevel::Minimal => "Minimal",
            ThinkLevel::Low => "Low",
            ThinkLevel::Medium => "Medium",
            ThinkLevel::High => "High",
            ThinkLevel::XHigh => "Extended",
        }
    }

    /// Get description for UI
    pub fn description(&self) -> &'static str {
        match self {
            ThinkLevel::Off => "No extended thinking, fastest responses",
            ThinkLevel::Minimal => "Brief internal reasoning",
            ThinkLevel::Low => "Basic thinking process",
            ThinkLevel::Medium => "Balanced thinking depth",
            ThinkLevel::High => "Detailed reasoning and analysis",
            ThinkLevel::XHigh => "Deep extended thinking (model-specific)",
        }
    }

    /// Get the recommended token budget for this thinking level
    ///
    /// These are approximate values that can be adjusted per provider.
    pub fn token_budget(&self) -> u32 {
        match self {
            ThinkLevel::Off => 0,
            ThinkLevel::Minimal => 1024,
            ThinkLevel::Low => 2048,
            ThinkLevel::Medium => 4096,
            ThinkLevel::High => 8192,
            ThinkLevel::XHigh => 16384,
        }
    }
}

impl std::fmt::Display for ThinkLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name().to_lowercase())
    }
}

impl std::str::FromStr for ThinkLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        normalize_think_level(s).ok_or_else(|| format!("Unknown thinking level: '{}'", s))
    }
}

// =============================================================================
// User Input Normalization
// =============================================================================

/// Normalize user-provided thinking level strings to canonical enum
///
/// Supports various aliases for user convenience:
/// - "think", "on", "enable" -> Minimal
/// - "thinkhard", "think-hard" -> Low
/// - "thinkharder", "harder" -> Medium
/// - "ultrathink", "ultra", "max" -> High
/// - "xhigh", "x-high", "extended" -> XHigh
///
/// # Arguments
///
/// * `raw` - Raw user input string
///
/// # Returns
///
/// * `Some(ThinkLevel)` - Successfully parsed level
/// * `None` - Unknown input
///
/// # Examples
///
/// ```rust
/// use alephcore::agents::thinking::{normalize_think_level, ThinkLevel};
///
/// assert_eq!(normalize_think_level("think"), Some(ThinkLevel::Minimal));
/// assert_eq!(normalize_think_level("ultrathink"), Some(ThinkLevel::High));
/// assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
/// assert_eq!(normalize_think_level("unknown"), None);
/// ```
pub fn normalize_think_level(raw: &str) -> Option<ThinkLevel> {
    let key = raw.trim().to_lowercase();

    match key.as_str() {
        // Off
        "off" | "none" | "disable" | "disabled" | "0" | "false" | "no" => Some(ThinkLevel::Off),

        // Minimal
        "minimal" | "min" | "think" | "on" | "enable" | "enabled" | "1" | "true" | "yes" => {
            Some(ThinkLevel::Minimal)
        }

        // Low
        "low" | "thinkhard" | "think-hard" | "think_hard" | "2" => Some(ThinkLevel::Low),

        // Medium
        "medium" | "med" | "mid" | "thinkharder" | "think-harder" | "think_harder" | "harder"
        | "3" => Some(ThinkLevel::Medium),

        // High
        "high" | "ultra" | "ultrathink" | "ultra-think" | "ultra_think" | "thinkhardest"
        | "think-hardest" | "highest" | "max" | "4" => Some(ThinkLevel::High),

        // XHigh
        "xhigh" | "x-high" | "x_high" | "extended" | "ext" | "5" | "extreme" => {
            Some(ThinkLevel::XHigh)
        }

        _ => None,
    }
}

/// List available thinking level labels for a provider/model combination
///
/// Binary thinking providers (like Z.AI) only support "off" and "on".
/// Models that support extended thinking include "xhigh".
pub fn list_thinking_level_labels(provider: &str, model: &str) -> Vec<&'static str> {
    if is_binary_thinking_provider(provider) {
        vec!["off", "on"]
    } else {
        let mut levels = vec!["off", "minimal", "low", "medium", "high"];
        if supports_xhigh_thinking(provider, model) {
            levels.push("xhigh");
        }
        levels
    }
}

/// Format thinking levels as comma-separated string
pub fn format_thinking_levels(provider: &str, model: &str) -> String {
    list_thinking_level_labels(provider, model).join(", ")
}

// =============================================================================
// Model Capability Matrix
// =============================================================================

/// Models that support xhigh (extended) thinking
///
/// Format: "provider/model" or just "model-id"
const XHIGH_MODEL_REFS: &[&str] = &[
    // OpenAI models with extended thinking
    "openai/gpt-5.2",
    "openai/o1",
    "openai/o1-preview",
    "openai/o1-mini",
    "openai/o3",
    "openai/o3-mini",
    // Anthropic models with extended thinking
    "claude/claude-opus-4-5-20251101",
    "claude/claude-3-opus-20240229",
    "anthropic/claude-opus-4-5-20251101",
    "anthropic/claude-3-opus-20240229",
];

/// Model IDs (without provider prefix) that support xhigh
const XHIGH_MODEL_IDS: &[&str] = &[
    "gpt-5.2",
    "o1",
    "o1-preview",
    "o1-mini",
    "o3",
    "o3-mini",
    "claude-opus-4-5-20251101",
    "claude-3-opus-20240229",
];

/// Providers that only support binary thinking (on/off)
const BINARY_THINKING_PROVIDERS: &[&str] = &["z.ai", "zai", "z-ai"];

/// Check if provider only supports binary thinking (on/off)
///
/// Some providers like Z.AI only support enabling or disabling thinking,
/// without granular level control.
pub fn is_binary_thinking_provider(provider: &str) -> bool {
    let normalized = provider.trim().to_lowercase();
    BINARY_THINKING_PROVIDERS.contains(&normalized.as_str())
}

/// Check if model supports xhigh (extended) thinking
///
/// Extended thinking is only available on specific high-capability models
/// like Claude Opus and OpenAI o1/o3 series.
pub fn supports_xhigh_thinking(provider: &str, model: &str) -> bool {
    let model_key = model.trim().to_lowercase();
    let provider_key = provider.trim().to_lowercase();

    // Check full reference (provider/model)
    let full_ref = format!("{}/{}", provider_key, model_key);
    if XHIGH_MODEL_REFS
        .iter()
        .any(|r| r.to_lowercase() == full_ref)
    {
        return true;
    }

    // Check model ID only
    XHIGH_MODEL_IDS
        .iter()
        .any(|id| id.to_lowercase() == model_key)
}

/// Get supported thinking levels for a provider/model combination
pub fn get_supported_levels(provider: &str, model: &str) -> Vec<ThinkLevel> {
    if is_binary_thinking_provider(provider) {
        vec![ThinkLevel::Off, ThinkLevel::Minimal]
    } else {
        let mut levels = vec![
            ThinkLevel::Off,
            ThinkLevel::Minimal,
            ThinkLevel::Low,
            ThinkLevel::Medium,
            ThinkLevel::High,
        ];
        if supports_xhigh_thinking(provider, model) {
            levels.push(ThinkLevel::XHigh);
        }
        levels
    }
}

/// Check if a thinking level is supported by provider/model
pub fn is_level_supported(level: ThinkLevel, provider: &str, model: &str) -> bool {
    get_supported_levels(provider, model).contains(&level)
}

// =============================================================================
// ThinkingConfig
// =============================================================================

/// Configuration for thinking level in LLM requests
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct ThinkingConfig {
    /// Requested thinking level
    pub level: ThinkLevel,
    /// Provider name (for capability checking)
    pub provider: String,
    /// Model name (for capability checking)
    pub model: String,
}

impl ThinkingConfig {
    /// Create a new ThinkingConfig
    pub fn new(level: ThinkLevel, provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            level,
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Get the effective level (capped by model capability)
    ///
    /// If the requested level is not supported, returns the highest
    /// supported level that is less than or equal to the requested level.
    pub fn effective_level(&self) -> ThinkLevel {
        let supported = get_supported_levels(&self.provider, &self.model);
        if supported.contains(&self.level) {
            self.level
        } else {
            // Find highest supported level <= requested
            supported
                .into_iter()
                .filter(|l| l.weight() <= self.level.weight())
                .max_by_key(|l| l.weight())
                .unwrap_or(ThinkLevel::Off)
        }
    }

    /// Check if the config is using full capability
    pub fn is_at_full_capability(&self) -> bool {
        self.level == self.effective_level()
    }

    /// Get the token budget for the effective level
    pub fn token_budget(&self) -> u32 {
        self.effective_level().token_budget()
    }
}


// =============================================================================
// ThinkingFallbackState
// =============================================================================

/// State tracker for thinking level fallback attempts
///
/// Tracks which levels have been attempted and manages automatic
/// downgrade when a level is not supported by the model.
#[derive(Debug, Default)]
pub struct ThinkingFallbackState {
    /// Set of already-attempted thinking levels
    pub attempted: HashSet<ThinkLevel>,
    /// Current thinking level
    pub current: ThinkLevel,
    /// Number of fallback attempts
    pub attempts: u32,
    /// Maximum fallback attempts before giving up
    pub max_attempts: u32,
}

impl ThinkingFallbackState {
    /// Create a new fallback state with initial level
    pub fn new(initial: ThinkLevel) -> Self {
        let mut attempted = HashSet::new();
        attempted.insert(initial);
        Self {
            attempted,
            current: initial,
            attempts: 0,
            max_attempts: 5,
        }
    }

    /// Create with custom max attempts
    pub fn with_max_attempts(mut self, max: u32) -> Self {
        self.max_attempts = max;
        self
    }

    /// Try to fallback to a lower thinking level
    ///
    /// First tries to parse supported levels from error message.
    /// If that fails, falls back to the next lower level.
    ///
    /// Returns the new level if fallback is possible, None otherwise.
    pub fn try_fallback(&mut self, error_message: Option<&str>) -> Option<ThinkLevel> {
        if self.is_exhausted() {
            return None;
        }

        // First try to parse supported levels from error message
        if let Some(level) = pick_fallback_from_error(error_message, &self.attempted) {
            self.attempted.insert(level);
            self.current = level;
            self.attempts += 1;
            return Some(level);
        }

        // Fall back to next lower level
        if let Some(lower) = self.current.fallback() {
            if !self.attempted.contains(&lower) {
                self.attempted.insert(lower);
                self.current = lower;
                self.attempts += 1;
                return Some(lower);
            }
        }

        None
    }

    /// Check if we've exhausted all fallback options
    pub fn is_exhausted(&self) -> bool {
        self.current == ThinkLevel::Off || self.attempts >= self.max_attempts
    }

    /// Reset the fallback state with a new initial level
    pub fn reset(&mut self, level: ThinkLevel) {
        self.attempted.clear();
        self.attempted.insert(level);
        self.current = level;
        self.attempts = 0;
    }
}

// =============================================================================
// Error Message Parsing
// =============================================================================

/// Extract supported thinking levels from error message
///
/// Parses error messages like:
/// - "supported values are: 'off', 'minimal', 'low'"
/// - "Supported values: off, low, medium"
fn extract_supported_values(message: &str) -> Vec<String> {
    // Pattern: "supported values are: ..." or "Supported values: ..."
    let lower = message.to_lowercase();

    // Find the "supported values" portion
    let start_idx = if let Some(idx) = lower.find("supported values are:") {
        idx + "supported values are:".len()
    } else if let Some(idx) = lower.find("supported values:") {
        idx + "supported values:".len()
    } else {
        return Vec::new();
    };

    // Extract until end of line or period
    let fragment = &message[start_idx..];
    let end_idx = fragment
        .find('\n')
        .or_else(|| fragment.find('.'))
        .unwrap_or(fragment.len());
    let text = &fragment[..end_idx];

    // Try to extract quoted values first
    let mut quoted = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    let mut quote_char = '"';

    for c in text.chars() {
        if !in_quote && (c == '"' || c == '\'') {
            in_quote = true;
            quote_char = c;
        } else if in_quote && c == quote_char {
            in_quote = false;
            if !current.trim().is_empty() {
                quoted.push(current.trim().to_string());
            }
            current.clear();
        } else if in_quote {
            current.push(c);
        }
    }

    if !quoted.is_empty() {
        return quoted;
    }

    // Fall back to comma/and separated values
    text.split([',', ' '])
        .map(|s| {
            s.trim()
                .trim_matches(|c: char| !c.is_alphabetic())
                .to_string()
        })
        .filter(|s| !s.is_empty() && s != "and" && s != "or")
        .collect()
}

/// Pick a fallback thinking level based on error message
///
/// Parses the error message to find supported levels, then returns
/// the first supported level that hasn't been attempted yet.
fn pick_fallback_from_error(
    message: Option<&str>,
    attempted: &HashSet<ThinkLevel>,
) -> Option<ThinkLevel> {
    let message = message?.trim();
    if message.is_empty() {
        return None;
    }

    let supported = extract_supported_values(message);
    if supported.is_empty() {
        return None;
    }

    for entry in supported {
        if let Some(level) = normalize_think_level(&entry) {
            if !attempted.contains(&level) {
                return Some(level);
            }
        }
    }

    None
}

/// Detect if error is related to unsupported thinking level
pub fn is_thinking_level_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("thinking")
        && (lower.contains("unsupported")
            || lower.contains("not supported")
            || lower.contains("invalid")
            || lower.contains("supported values"))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ThinkLevel tests
    #[test]
    fn test_think_level_weight() {
        assert_eq!(ThinkLevel::Off.weight(), 0);
        assert_eq!(ThinkLevel::Minimal.weight(), 1);
        assert_eq!(ThinkLevel::Low.weight(), 2);
        assert_eq!(ThinkLevel::Medium.weight(), 3);
        assert_eq!(ThinkLevel::High.weight(), 4);
        assert_eq!(ThinkLevel::XHigh.weight(), 5);
    }

    #[test]
    fn test_think_level_comparison() {
        assert!(ThinkLevel::High.is_higher_than(&ThinkLevel::Low));
        assert!(ThinkLevel::Low.is_lower_than(&ThinkLevel::High));
        assert!(!ThinkLevel::Medium.is_higher_than(&ThinkLevel::Medium));
    }

    #[test]
    fn test_think_level_fallback() {
        assert_eq!(ThinkLevel::XHigh.fallback(), Some(ThinkLevel::High));
        assert_eq!(ThinkLevel::High.fallback(), Some(ThinkLevel::Medium));
        assert_eq!(ThinkLevel::Medium.fallback(), Some(ThinkLevel::Low));
        assert_eq!(ThinkLevel::Low.fallback(), Some(ThinkLevel::Minimal));
        assert_eq!(ThinkLevel::Minimal.fallback(), Some(ThinkLevel::Off));
        assert_eq!(ThinkLevel::Off.fallback(), None);
    }

    #[test]
    fn test_think_level_default() {
        assert_eq!(ThinkLevel::default(), ThinkLevel::Minimal);
    }

    #[test]
    fn test_think_level_display() {
        assert_eq!(format!("{}", ThinkLevel::High), "high");
        assert_eq!(format!("{}", ThinkLevel::XHigh), "extended");
    }

    #[test]
    fn test_think_level_from_str() {
        assert_eq!("high".parse::<ThinkLevel>().unwrap(), ThinkLevel::High);
        assert_eq!("xhigh".parse::<ThinkLevel>().unwrap(), ThinkLevel::XHigh);
        assert!("unknown".parse::<ThinkLevel>().is_err());
    }

    // Normalization tests
    #[test]
    fn test_normalize_off() {
        assert_eq!(normalize_think_level("off"), Some(ThinkLevel::Off));
        assert_eq!(normalize_think_level("disable"), Some(ThinkLevel::Off));
        assert_eq!(normalize_think_level("0"), Some(ThinkLevel::Off));
        assert_eq!(normalize_think_level("false"), Some(ThinkLevel::Off));
    }

    #[test]
    fn test_normalize_minimal() {
        assert_eq!(normalize_think_level("minimal"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("think"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("on"), Some(ThinkLevel::Minimal));
        assert_eq!(normalize_think_level("1"), Some(ThinkLevel::Minimal));
    }

    #[test]
    fn test_normalize_low() {
        assert_eq!(normalize_think_level("low"), Some(ThinkLevel::Low));
        assert_eq!(normalize_think_level("thinkhard"), Some(ThinkLevel::Low));
        assert_eq!(normalize_think_level("think-hard"), Some(ThinkLevel::Low));
    }

    #[test]
    fn test_normalize_medium() {
        assert_eq!(normalize_think_level("medium"), Some(ThinkLevel::Medium));
        assert_eq!(normalize_think_level("med"), Some(ThinkLevel::Medium));
        assert_eq!(
            normalize_think_level("thinkharder"),
            Some(ThinkLevel::Medium)
        );
    }

    #[test]
    fn test_normalize_high() {
        assert_eq!(normalize_think_level("high"), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("ultrathink"), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("max"), Some(ThinkLevel::High));
    }

    #[test]
    fn test_normalize_xhigh() {
        assert_eq!(normalize_think_level("xhigh"), Some(ThinkLevel::XHigh));
        assert_eq!(normalize_think_level("x-high"), Some(ThinkLevel::XHigh));
        assert_eq!(normalize_think_level("extended"), Some(ThinkLevel::XHigh));
    }

    #[test]
    fn test_normalize_case_insensitive() {
        assert_eq!(normalize_think_level("HIGH"), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("UltraThink"), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("XHIGH"), Some(ThinkLevel::XHigh));
    }

    #[test]
    fn test_normalize_with_whitespace() {
        assert_eq!(normalize_think_level("  high  "), Some(ThinkLevel::High));
        assert_eq!(normalize_think_level("\tmedium\n"), Some(ThinkLevel::Medium));
    }

    #[test]
    fn test_normalize_unknown() {
        assert_eq!(normalize_think_level("unknown"), None);
        assert_eq!(normalize_think_level(""), None);
        assert_eq!(normalize_think_level("super"), None);
    }

    // Model capability tests
    #[test]
    fn test_supports_xhigh() {
        assert!(supports_xhigh_thinking("openai", "o1"));
        assert!(supports_xhigh_thinking("openai", "o1-preview"));
        assert!(supports_xhigh_thinking("claude", "claude-opus-4-5-20251101"));
        assert!(!supports_xhigh_thinking("openai", "gpt-4o"));
        assert!(!supports_xhigh_thinking("claude", "claude-3-5-sonnet-20241022"));
    }

    #[test]
    fn test_supports_xhigh_case_insensitive() {
        assert!(supports_xhigh_thinking("OPENAI", "O1"));
        assert!(supports_xhigh_thinking("Claude", "Claude-Opus-4-5-20251101"));
    }

    #[test]
    fn test_binary_thinking_provider() {
        assert!(is_binary_thinking_provider("z.ai"));
        assert!(is_binary_thinking_provider("zai"));
        assert!(is_binary_thinking_provider("Z.AI"));
        assert!(!is_binary_thinking_provider("openai"));
        assert!(!is_binary_thinking_provider("claude"));
    }

    #[test]
    fn test_get_supported_levels() {
        let levels = get_supported_levels("openai", "gpt-4o");
        assert!(levels.contains(&ThinkLevel::Off));
        assert!(levels.contains(&ThinkLevel::High));
        assert!(!levels.contains(&ThinkLevel::XHigh));

        let levels = get_supported_levels("openai", "o1");
        assert!(levels.contains(&ThinkLevel::XHigh));

        let levels = get_supported_levels("z.ai", "model");
        assert_eq!(levels.len(), 2);
        assert!(levels.contains(&ThinkLevel::Off));
        assert!(levels.contains(&ThinkLevel::Minimal));
    }

    #[test]
    fn test_is_level_supported() {
        assert!(is_level_supported(ThinkLevel::High, "openai", "gpt-4o"));
        assert!(!is_level_supported(ThinkLevel::XHigh, "openai", "gpt-4o"));
        assert!(is_level_supported(ThinkLevel::XHigh, "openai", "o1"));
    }

    // ThinkingConfig tests
    #[test]
    fn test_thinking_config_effective_level() {
        let config = ThinkingConfig::new(ThinkLevel::XHigh, "openai", "gpt-4o");
        assert_eq!(config.effective_level(), ThinkLevel::High);

        let config = ThinkingConfig::new(ThinkLevel::XHigh, "openai", "o1");
        assert_eq!(config.effective_level(), ThinkLevel::XHigh);
    }

    #[test]
    fn test_thinking_config_token_budget() {
        let config = ThinkingConfig::new(ThinkLevel::High, "openai", "gpt-4o");
        assert_eq!(config.token_budget(), 8192);

        let config = ThinkingConfig::new(ThinkLevel::XHigh, "openai", "gpt-4o");
        // XHigh not supported, falls back to High
        assert_eq!(config.token_budget(), 8192);
    }

    // Fallback state tests
    #[test]
    fn test_fallback_state_basic() {
        let mut state = ThinkingFallbackState::new(ThinkLevel::High);
        assert_eq!(state.current, ThinkLevel::High);
        assert!(!state.is_exhausted());

        let next = state.try_fallback(None);
        assert_eq!(next, Some(ThinkLevel::Medium));
        assert_eq!(state.current, ThinkLevel::Medium);
        assert_eq!(state.attempts, 1);
    }

    #[test]
    fn test_fallback_state_exhausted() {
        let mut state = ThinkingFallbackState::new(ThinkLevel::Minimal);
        assert!(!state.is_exhausted());

        let next = state.try_fallback(None);
        assert_eq!(next, Some(ThinkLevel::Off));
        assert!(state.is_exhausted());

        let next = state.try_fallback(None);
        assert_eq!(next, None);
    }

    #[test]
    fn test_fallback_state_max_attempts() {
        let mut state = ThinkingFallbackState::new(ThinkLevel::XHigh).with_max_attempts(2);

        state.try_fallback(None); // 1
        state.try_fallback(None); // 2
        assert!(state.is_exhausted());
    }

    #[test]
    fn test_fallback_state_reset() {
        let mut state = ThinkingFallbackState::new(ThinkLevel::High);
        state.try_fallback(None);
        state.try_fallback(None);

        state.reset(ThinkLevel::XHigh);
        assert_eq!(state.current, ThinkLevel::XHigh);
        assert_eq!(state.attempts, 0);
        assert!(!state.is_exhausted());
    }

    // Error parsing tests
    #[test]
    fn test_extract_supported_values_quoted() {
        let msg = "Error: unsupported thinking level. Supported values are: 'off', 'minimal', 'low'";
        let values = extract_supported_values(msg);
        assert_eq!(values, vec!["off", "minimal", "low"]);
    }

    #[test]
    fn test_extract_supported_values_unquoted() {
        let msg = "Invalid thinking level. Supported values: off, low, medium, high";
        let values = extract_supported_values(msg);
        assert!(values.contains(&"off".to_string()));
        assert!(values.contains(&"low".to_string()));
        assert!(values.contains(&"medium".to_string()));
        assert!(values.contains(&"high".to_string()));
    }

    #[test]
    fn test_extract_supported_values_empty() {
        let msg = "Some other error message";
        let values = extract_supported_values(msg);
        assert!(values.is_empty());
    }

    #[test]
    fn test_pick_fallback_from_error() {
        let msg = "Supported values are: 'off', 'minimal', 'low'";
        let mut attempted = HashSet::new();
        attempted.insert(ThinkLevel::High);

        let fallback = pick_fallback_from_error(Some(msg), &attempted);
        assert!(fallback.is_some());
        let level = fallback.unwrap();
        assert!(level.weight() < ThinkLevel::High.weight());
    }

    #[test]
    fn test_pick_fallback_all_attempted() {
        let msg = "Supported values are: 'off', 'minimal'";
        let mut attempted = HashSet::new();
        attempted.insert(ThinkLevel::Off);
        attempted.insert(ThinkLevel::Minimal);

        let fallback = pick_fallback_from_error(Some(msg), &attempted);
        assert!(fallback.is_none());
    }

    #[test]
    fn test_is_thinking_level_error() {
        assert!(is_thinking_level_error(
            "thinking level not supported for this model"
        ));
        assert!(is_thinking_level_error("invalid thinking parameter"));
        assert!(is_thinking_level_error(
            "Thinking: Supported values are: off, low"
        ));
        assert!(!is_thinking_level_error("rate limit exceeded"));
        assert!(!is_thinking_level_error("authentication failed"));
    }

    // Serialization tests
    #[test]
    fn test_think_level_serialization() {
        let level = ThinkLevel::High;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"high\"");

        let parsed: ThinkLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ThinkLevel::High);
    }

    #[test]
    fn test_think_level_all_serialization() {
        for level in ThinkLevel::ALL {
            let json = serde_json::to_string(level).unwrap();
            let parsed: ThinkLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*level, parsed);
        }
    }
}
