//! Confidence Calibration Module
//!
//! Provides layer-specific and history-based confidence calibration for intent signals.
//!
//! # Overview
//!
//! The calibrator adjusts raw confidence scores based on:
//! - **Layer dampening**: L2/L3 signals get dampened (L1 regex matches are trusted)
//! - **History boost**: Patterns with high success rates get boosted
//! - **Context boost**: Recently used tools get a small confidence boost
//!
//! # Example
//!
//! ```ignore
//! use alephcore::intent::{ConfidenceCalibrator, CalibratorConfig, RoutingLayer, IntentSignal};
//!
//! let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
//!
//! let signal = IntentSignal {
//!     intent_type: "search".to_string(),
//!     tool_name: Some("search".to_string()),
//!     confidence: 0.85,
//!     parameters: HashMap::new(),
//! };
//!
//! let calibrated = calibrator.calibrate(signal, RoutingLayer::L2Keyword, &[]);
//! // calibrated.calibrated_confidence will be 0.85 * 0.9 = 0.765
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

// =============================================================================
// RoutingLayer
// =============================================================================

/// Routing layer types for confidence calibration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutingLayer {
    /// L1: Exact regex pattern match (confidence 1.0, no dampening)
    L1Regex,
    /// L2: Weighted keyword match (dampened by l2_dampening)
    L2Keyword,
    /// L3: AI-based detection (dampened by l3_correction)
    L3Ai,
}

impl RoutingLayer {
    /// Returns the layer name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            RoutingLayer::L1Regex => "L1_Regex",
            RoutingLayer::L2Keyword => "L2_Keyword",
            RoutingLayer::L3Ai => "L3_AI",
        }
    }
}

// =============================================================================
// PatternStats (Private)
// =============================================================================

/// Statistics for pattern learning (private)
#[derive(Debug, Clone)]
struct PatternStats {
    /// Total successful executions
    successes: u32,
    /// Total failed/cancelled executions
    failures: u32,
    /// Timestamp of last access (for LRU pruning)
    last_access: Instant,
}

impl Default for PatternStats {
    fn default() -> Self {
        Self {
            successes: 0,
            failures: 0,
            last_access: Instant::now(),
        }
    }
}

// =============================================================================
// CalibrationHistory
// =============================================================================

/// Calibration history for pattern learning
///
/// Tracks success/failure rates for tool + input pattern combinations
/// to provide history-based confidence boosts.
#[derive(Debug, Default)]
pub struct CalibrationHistory {
    /// Success/failure counts per "tool:normalized_input" key
    tool_patterns: HashMap<String, PatternStats>,
    /// Maximum entries to keep (LRU eviction)
    max_entries: usize,
}

impl CalibrationHistory {
    /// Create a new history with specified max entries
    pub fn new(max_entries: usize) -> Self {
        Self {
            tool_patterns: HashMap::new(),
            max_entries,
        }
    }

    /// Record a successful execution for a tool + input pattern
    pub fn record_success(&mut self, tool: &str, input: &str) {
        let key = format!("{}:{}", tool, Self::normalize_input(input));
        let stats = self.tool_patterns.entry(key).or_default();
        stats.successes += 1;
        stats.last_access = Instant::now();
        self.prune_if_needed();
    }

    /// Record a failed/cancelled execution for a tool + input pattern
    pub fn record_failure(&mut self, tool: &str, input: &str) {
        let key = format!("{}:{}", tool, Self::normalize_input(input));
        let stats = self.tool_patterns.entry(key).or_default();
        stats.failures += 1;
        stats.last_access = Instant::now();
        self.prune_if_needed();
    }

    /// Get success rate for a tool + input pattern
    ///
    /// Returns `Some(rate)` where rate is 0.0-1.0, or `None` if no history exists.
    pub fn get_success_rate(&self, tool: &str, input: &str) -> Option<f32> {
        let key = format!("{}:{}", tool, Self::normalize_input(input));
        self.tool_patterns.get(&key).map(|stats| {
            let total = stats.successes + stats.failures;
            if total == 0 {
                1.0 // No data yet, assume success
            } else {
                stats.successes as f32 / total as f32
            }
        })
    }

    /// Clear all history
    pub fn clear(&mut self) {
        self.tool_patterns.clear();
    }

    /// Normalize input for consistent pattern matching
    ///
    /// - Converts to lowercase
    /// - Trims whitespace
    /// - Truncates to first 50 characters
    fn normalize_input(input: &str) -> String {
        let normalized = input.trim().to_lowercase();
        // UTF-8 safe truncation: limit to 25 characters
        if normalized.chars().count() > 25 {
            let end_byte = normalized
                .char_indices()
                .nth(25)
                .map(|(i, _)| i)
                .unwrap_or(normalized.len());
            normalized[..end_byte].to_string()
        } else {
            normalized
        }
    }

    /// Remove oldest entry if over capacity
    fn prune_if_needed(&mut self) {
        if self.tool_patterns.len() > self.max_entries {
            // Find the oldest entry by last_access time
            if let Some((oldest_key, _)) = self
                .tool_patterns
                .iter()
                .min_by_key(|(_, v)| v.last_access)
                .map(|(k, v)| (k.clone(), v.clone()))
            {
                self.tool_patterns.remove(&oldest_key);
            }
        }
    }

    /// Get current entry count
    pub fn len(&self) -> usize {
        self.tool_patterns.len()
    }

    /// Check if history is empty
    pub fn is_empty(&self) -> bool {
        self.tool_patterns.is_empty()
    }
}

// =============================================================================
// IntentSignal
// =============================================================================

/// Input signal for calibration
#[derive(Debug, Clone)]
pub struct IntentSignal {
    /// The classified intent type (e.g., "search", "file_organize")
    pub intent_type: String,
    /// The tool name if a specific tool was matched
    pub tool_name: Option<String>,
    /// Raw confidence score before calibration (0.0-1.0)
    pub confidence: f32,
    /// Extracted parameters from the input
    pub parameters: HashMap<String, String>,
}

impl IntentSignal {
    /// Create a new intent signal
    pub fn new(intent_type: impl Into<String>, confidence: f32) -> Self {
        Self {
            intent_type: intent_type.into(),
            tool_name: None,
            confidence,
            parameters: HashMap::new(),
        }
    }

    /// Create a signal with a tool name
    pub fn with_tool(
        intent_type: impl Into<String>,
        tool_name: impl Into<String>,
        confidence: f32,
    ) -> Self {
        Self {
            intent_type: intent_type.into(),
            tool_name: Some(tool_name.into()),
            confidence,
            parameters: HashMap::new(),
        }
    }

    /// Add a parameter
    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.parameters.insert(key.into(), value.into());
        self
    }
}

// =============================================================================
// CalibratedSignal
// =============================================================================

/// Wrapper with calibrated confidence
#[derive(Debug, Clone)]
pub struct CalibratedSignal {
    /// The classified intent type
    pub intent_type: String,
    /// The tool name if a specific tool was matched
    pub tool_name: Option<String>,
    /// Original confidence before calibration
    pub original_confidence: f32,
    /// Calibrated confidence after adjustments
    pub calibrated_confidence: f32,
    /// The routing layer that produced this signal
    pub layer: RoutingLayer,
}

impl CalibratedSignal {
    /// Create from an intent signal with calibrated confidence
    pub fn from_signal(
        signal: &IntentSignal,
        calibrated_confidence: f32,
        layer: RoutingLayer,
    ) -> Self {
        Self {
            intent_type: signal.intent_type.clone(),
            tool_name: signal.tool_name.clone(),
            original_confidence: signal.confidence,
            calibrated_confidence,
            layer,
        }
    }

    /// Check if the signal meets a confidence threshold
    pub fn meets_threshold(&self, threshold: f32) -> bool {
        self.calibrated_confidence >= threshold
    }

    /// Get the confidence delta from calibration
    pub fn confidence_delta(&self) -> f32 {
        self.calibrated_confidence - self.original_confidence
    }
}

// =============================================================================
// CalibratorConfig
// =============================================================================

/// Configuration for the confidence calibrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibratorConfig {
    /// Multiplier for L2 keyword match signals (default: 0.9)
    #[serde(default = "default_l2_dampening")]
    pub l2_dampening: f32,

    /// Multiplier for L3 AI detection signals (default: 0.95)
    #[serde(default = "default_l3_correction")]
    pub l3_correction: f32,

    /// Maximum confidence boost from recent tool usage (default: 0.15)
    #[serde(default = "default_max_context_boost")]
    pub max_context_boost: f32,

    /// Confidence boost per recent tool use (default: 0.05)
    #[serde(default = "default_recent_use_boost")]
    pub recent_use_boost: f32,

    /// Maximum entries in history (default: 500)
    #[serde(default = "default_history_max_entries")]
    pub history_max_entries: usize,
}

fn default_l2_dampening() -> f32 {
    0.9
}

fn default_l3_correction() -> f32 {
    0.95
}

fn default_max_context_boost() -> f32 {
    0.15
}

fn default_recent_use_boost() -> f32 {
    0.05
}

fn default_history_max_entries() -> usize {
    500
}

impl Default for CalibratorConfig {
    fn default() -> Self {
        Self {
            l2_dampening: default_l2_dampening(),
            l3_correction: default_l3_correction(),
            max_context_boost: default_max_context_boost(),
            recent_use_boost: default_recent_use_boost(),
            history_max_entries: default_history_max_entries(),
        }
    }
}

// =============================================================================
// ConfidenceCalibrator
// =============================================================================

/// Main confidence calibrator
///
/// Calibrates intent signal confidence based on:
/// 1. Layer-specific dampening (L2, L3)
/// 2. History-based boost (high success rate patterns)
/// 3. Context-based boost (recently used tools)
pub struct ConfidenceCalibrator {
    /// Configuration
    config: CalibratorConfig,
    /// Calibration history for learning
    history: Arc<RwLock<CalibrationHistory>>,
}

impl ConfidenceCalibrator {
    /// Create a new calibrator with the given configuration
    pub fn new(config: CalibratorConfig) -> Self {
        let history = CalibrationHistory::new(config.history_max_entries);
        Self {
            config,
            history: Arc::new(RwLock::new(history)),
        }
    }

    /// Calibrate an intent signal
    ///
    /// Applies:
    /// 1. Layer dampening (L2 * l2_dampening, L3 * l3_correction)
    /// 2. History boost if success_rate > 0.8
    /// 3. Context boost from recent tool usage
    pub fn calibrate(
        &self,
        signal: IntentSignal,
        layer: RoutingLayer,
        recent_tools: &[String],
    ) -> CalibratedSignal {
        let mut confidence = signal.confidence;

        // 1. Apply layer-specific dampening
        confidence = self.apply_layer_dampening(confidence, layer);

        // 2. Apply history boost (sync version - uses try_read)
        confidence = self.apply_history_boost_sync(&signal, confidence);

        // 3. Apply context boost from recent tool usage
        confidence = self.apply_context_boost(&signal, confidence, recent_tools);

        // Clamp to [0.0, 1.0]
        confidence = confidence.clamp(0.0, 1.0);

        CalibratedSignal::from_signal(&signal, confidence, layer)
    }

    /// Calibrate with async history access
    pub async fn calibrate_async(
        &self,
        signal: IntentSignal,
        layer: RoutingLayer,
        recent_tools: &[String],
    ) -> CalibratedSignal {
        let mut confidence = signal.confidence;

        // 1. Apply layer-specific dampening
        confidence = self.apply_layer_dampening(confidence, layer);

        // 2. Apply history boost (async version)
        confidence = self.apply_history_boost_async(&signal, confidence).await;

        // 3. Apply context boost from recent tool usage
        confidence = self.apply_context_boost(&signal, confidence, recent_tools);

        // Clamp to [0.0, 1.0]
        confidence = confidence.clamp(0.0, 1.0);

        CalibratedSignal::from_signal(&signal, confidence, layer)
    }

    /// Record a successful execution for learning
    pub async fn record_success(&self, tool: &str, input: &str) {
        let mut history = self.history.write().await;
        history.record_success(tool, input);
    }

    /// Record a failed/cancelled execution for learning
    pub async fn record_failure(&self, tool: &str, input: &str) {
        let mut history = self.history.write().await;
        history.record_failure(tool, input);
    }

    /// Get the current configuration
    pub fn config(&self) -> &CalibratorConfig {
        &self.config
    }

    /// Get a reference to the history (for testing/inspection)
    pub fn history(&self) -> &Arc<RwLock<CalibrationHistory>> {
        &self.history
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    /// Apply layer-specific dampening
    fn apply_layer_dampening(&self, confidence: f32, layer: RoutingLayer) -> f32 {
        match layer {
            RoutingLayer::L1Regex => {
                // L1 regex matches are exact - no dampening
                confidence
            }
            RoutingLayer::L2Keyword => {
                // L2 keyword matches may be over-confident
                confidence * self.config.l2_dampening
            }
            RoutingLayer::L3Ai => {
                // L3 AI detection can be overconfident
                confidence * self.config.l3_correction
            }
        }
    }

    /// Apply history boost (sync version using try_read)
    fn apply_history_boost_sync(&self, signal: &IntentSignal, confidence: f32) -> f32 {
        if let Some(tool_name) = &signal.tool_name {
            // Try to get read lock without blocking
            if let Ok(history) = self.history.try_read() {
                if let Some(success_rate) = history.get_success_rate(tool_name, &signal.intent_type)
                {
                    if success_rate > 0.8 {
                        // Boost by up to 0.1, but don't exceed 1.0
                        let boost = (1.0 - confidence).min(0.1);
                        return confidence + boost;
                    }
                }
            }
        }
        confidence
    }

    /// Apply history boost (async version)
    async fn apply_history_boost_async(&self, signal: &IntentSignal, confidence: f32) -> f32 {
        if let Some(tool_name) = &signal.tool_name {
            let history = self.history.read().await;
            if let Some(success_rate) = history.get_success_rate(tool_name, &signal.intent_type) {
                if success_rate > 0.8 {
                    // Boost by up to 0.1, but don't exceed 1.0
                    let boost = (1.0 - confidence).min(0.1);
                    return confidence + boost;
                }
            }
        }
        confidence
    }

    /// Apply context boost from recent tool usage
    fn apply_context_boost(
        &self,
        signal: &IntentSignal,
        confidence: f32,
        recent_tools: &[String],
    ) -> f32 {
        if let Some(tool_name) = &signal.tool_name {
            // Count how many times this tool appears in recent usage
            let match_count = recent_tools
                .iter()
                .filter(|t| t.eq_ignore_ascii_case(tool_name))
                .count();

            if match_count > 0 {
                // Apply boost per match, up to max_context_boost
                let boost = (match_count as f32 * self.config.recent_use_boost)
                    .min(self.config.max_context_boost);
                return confidence + boost;
            }
        }
        confidence
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // RoutingLayer tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_routing_layer_as_str() {
        assert_eq!(RoutingLayer::L1Regex.as_str(), "L1_Regex");
        assert_eq!(RoutingLayer::L2Keyword.as_str(), "L2_Keyword");
        assert_eq!(RoutingLayer::L3Ai.as_str(), "L3_AI");
    }

    #[test]
    fn test_routing_layer_equality() {
        assert_eq!(RoutingLayer::L1Regex, RoutingLayer::L1Regex);
        assert_ne!(RoutingLayer::L1Regex, RoutingLayer::L2Keyword);
    }

    // -------------------------------------------------------------------------
    // CalibrationHistory tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_calibration_history_new() {
        let history = CalibrationHistory::new(100);
        assert_eq!(history.max_entries, 100);
        assert!(history.is_empty());
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn test_calibration_history_record_success() {
        let mut history = CalibrationHistory::new(100);
        history.record_success("search", "test query");
        history.record_success("search", "test query");

        let rate = history.get_success_rate("search", "test query");
        assert!(rate.is_some());
        assert!((rate.unwrap() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_calibration_history_record_failure() {
        let mut history = CalibrationHistory::new(100);
        history.record_success("search", "test query");
        history.record_success("search", "test query");
        history.record_failure("search", "test query");

        let rate = history.get_success_rate("search", "test query");
        assert!(rate.is_some());
        // 2 successes, 1 failure = 2/3 = 0.666...
        assert!((rate.unwrap() - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_calibration_history_no_entry() {
        let history = CalibrationHistory::new(100);
        let rate = history.get_success_rate("nonexistent", "query");
        assert!(rate.is_none());
    }

    #[test]
    fn test_calibration_history_clear() {
        let mut history = CalibrationHistory::new(100);
        history.record_success("search", "test");
        history.record_success("browse", "test");
        assert_eq!(history.len(), 2);

        history.clear();
        assert!(history.is_empty());
    }

    #[test]
    fn test_calibration_history_normalize_input() {
        let mut history = CalibrationHistory::new(100);

        // Same input with different casing/whitespace should match
        history.record_success("search", "  Test Query  ");
        let rate = history.get_success_rate("search", "test query");
        assert!(rate.is_some());
    }

    #[test]
    fn test_calibration_history_truncation() {
        let mut history = CalibrationHistory::new(100);

        // Long input should be truncated to 50 chars
        let long_input = "a".repeat(100);
        history.record_success("search", &long_input);

        // Should still find it with the same long input
        let rate = history.get_success_rate("search", &long_input);
        assert!(rate.is_some());
    }

    #[test]
    fn test_calibration_history_pruning() {
        let mut history = CalibrationHistory::new(3);

        // Add more entries than max
        history.record_success("tool1", "input1");
        std::thread::sleep(std::time::Duration::from_millis(10));
        history.record_success("tool2", "input2");
        std::thread::sleep(std::time::Duration::from_millis(10));
        history.record_success("tool3", "input3");
        std::thread::sleep(std::time::Duration::from_millis(10));
        history.record_success("tool4", "input4");

        // Should have pruned to max size
        assert!(history.len() <= 3);
    }

    // -------------------------------------------------------------------------
    // IntentSignal tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_intent_signal_new() {
        let signal = IntentSignal::new("search", 0.85);
        assert_eq!(signal.intent_type, "search");
        assert_eq!(signal.confidence, 0.85);
        assert!(signal.tool_name.is_none());
        assert!(signal.parameters.is_empty());
    }

    #[test]
    fn test_intent_signal_with_tool() {
        let signal = IntentSignal::with_tool("search", "web_search", 0.9);
        assert_eq!(signal.intent_type, "search");
        assert_eq!(signal.tool_name, Some("web_search".to_string()));
        assert_eq!(signal.confidence, 0.9);
    }

    #[test]
    fn test_intent_signal_with_param() {
        let signal = IntentSignal::new("search", 0.85)
            .with_param("query", "test")
            .with_param("limit", "10");
        assert_eq!(signal.parameters.get("query"), Some(&"test".to_string()));
        assert_eq!(signal.parameters.get("limit"), Some(&"10".to_string()));
    }

    // -------------------------------------------------------------------------
    // CalibratedSignal tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_calibrated_signal_from_signal() {
        let signal = IntentSignal::with_tool("search", "web_search", 0.9);
        let calibrated = CalibratedSignal::from_signal(&signal, 0.81, RoutingLayer::L2Keyword);

        assert_eq!(calibrated.intent_type, "search");
        assert_eq!(calibrated.tool_name, Some("web_search".to_string()));
        assert_eq!(calibrated.original_confidence, 0.9);
        assert_eq!(calibrated.calibrated_confidence, 0.81);
        assert_eq!(calibrated.layer, RoutingLayer::L2Keyword);
    }

    #[test]
    fn test_calibrated_signal_meets_threshold() {
        let signal = IntentSignal::new("search", 0.85);
        let calibrated = CalibratedSignal::from_signal(&signal, 0.765, RoutingLayer::L2Keyword);

        assert!(calibrated.meets_threshold(0.7));
        assert!(!calibrated.meets_threshold(0.8));
    }

    #[test]
    fn test_calibrated_signal_confidence_delta() {
        let signal = IntentSignal::new("search", 0.9);
        let calibrated = CalibratedSignal::from_signal(&signal, 0.81, RoutingLayer::L2Keyword);

        // Delta should be -0.09
        assert!((calibrated.confidence_delta() - (-0.09)).abs() < 0.001);
    }

    // -------------------------------------------------------------------------
    // CalibratorConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_calibrator_config_default() {
        let config = CalibratorConfig::default();
        assert_eq!(config.l2_dampening, 0.9);
        assert_eq!(config.l3_correction, 0.95);
        assert_eq!(config.max_context_boost, 0.15);
        assert_eq!(config.recent_use_boost, 0.05);
        assert_eq!(config.history_max_entries, 500);
    }

    #[test]
    fn test_calibrator_config_serialize() {
        let config = CalibratorConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("l2_dampening"));
        assert!(json.contains("0.9"));
    }

    #[test]
    fn test_calibrator_config_deserialize() {
        let json = r#"{"l2_dampening": 0.85, "l3_correction": 0.9}"#;
        let config: CalibratorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.l2_dampening, 0.85);
        assert_eq!(config.l3_correction, 0.9);
        // Defaults should fill in
        assert_eq!(config.max_context_boost, 0.15);
    }

    // -------------------------------------------------------------------------
    // ConfidenceCalibrator tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_calibrator_new() {
        let config = CalibratorConfig::default();
        let calibrator = ConfidenceCalibrator::new(config.clone());
        assert_eq!(calibrator.config().l2_dampening, config.l2_dampening);
    }

    #[test]
    fn test_calibrator_l1_no_dampening() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::with_tool("search", "web_search", 1.0);

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L1Regex, &[]);

        // L1 should have no dampening
        assert!((calibrated.calibrated_confidence - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_calibrator_l2_dampening() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::with_tool("search", "web_search", 0.9);

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L2Keyword, &[]);

        // L2 should be dampened: 0.9 * 0.9 = 0.81
        assert!((calibrated.calibrated_confidence - 0.81).abs() < 0.001);
    }

    #[test]
    fn test_calibrator_l3_correction() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::with_tool("search", "web_search", 0.9);

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L3Ai, &[]);

        // L3 should be corrected: 0.9 * 0.95 = 0.855
        assert!((calibrated.calibrated_confidence - 0.855).abs() < 0.001);
    }

    #[test]
    fn test_calibrator_context_boost() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::with_tool("search", "web_search", 0.8);
        let recent_tools = vec!["web_search".to_string(), "web_search".to_string()];

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L1Regex, &recent_tools);

        // Should have context boost: 0.8 + (2 * 0.05) = 0.9
        assert!((calibrated.calibrated_confidence - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_calibrator_context_boost_max() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::with_tool("search", "web_search", 0.8);
        // 10 recent uses would be 0.5 boost, but max is 0.15
        let recent_tools = vec!["web_search".to_string(); 10];

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L1Regex, &recent_tools);

        // Should be capped at max_context_boost: 0.8 + 0.15 = 0.95
        assert!((calibrated.calibrated_confidence - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_calibrator_clamping() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::with_tool("search", "web_search", 1.0);
        let recent_tools = vec!["web_search".to_string(); 10];

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L1Regex, &recent_tools);

        // Should be clamped to 1.0
        assert!(calibrated.calibrated_confidence <= 1.0);
    }

    #[test]
    fn test_calibrator_no_tool_name() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());
        let signal = IntentSignal::new("general_chat", 0.5);
        let recent_tools = vec!["web_search".to_string()];

        let calibrated = calibrator.calibrate(signal, RoutingLayer::L2Keyword, &recent_tools);

        // No tool name, so no context boost, just L2 dampening: 0.5 * 0.9 = 0.45
        assert!((calibrated.calibrated_confidence - 0.45).abs() < 0.001);
    }

    // -------------------------------------------------------------------------
    // Async tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_calibrator_record_success() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());

        calibrator.record_success("search", "test query").await;
        calibrator.record_success("search", "test query").await;

        let history = calibrator.history.read().await;
        let rate = history.get_success_rate("search", "test query");
        assert!(rate.is_some());
        assert!((rate.unwrap() - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_calibrator_record_failure() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());

        calibrator.record_success("search", "test query").await;
        calibrator.record_failure("search", "test query").await;

        let history = calibrator.history.read().await;
        let rate = history.get_success_rate("search", "test query");
        assert!(rate.is_some());
        assert!((rate.unwrap() - 0.5).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_calibrator_history_boost() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());

        // Record many successes to get > 0.8 success rate
        for _ in 0..10 {
            calibrator.record_success("web_search", "search").await;
        }

        let signal = IntentSignal::with_tool("search", "web_search", 0.8);
        let calibrated = calibrator
            .calibrate_async(signal, RoutingLayer::L1Regex, &[])
            .await;

        // Should have history boost: 0.8 + 0.1 = 0.9 (capped at min(1.0-0.8, 0.1) = 0.1)
        assert!(calibrated.calibrated_confidence > 0.8);
        assert!((calibrated.calibrated_confidence - 0.9).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_calibrator_history_no_boost_low_success_rate() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());

        // Record mixed results for < 0.8 success rate
        calibrator.record_success("web_search", "search").await;
        calibrator.record_failure("web_search", "search").await;
        calibrator.record_failure("web_search", "search").await;

        let signal = IntentSignal::with_tool("search", "web_search", 0.8);
        let calibrated = calibrator
            .calibrate_async(signal, RoutingLayer::L1Regex, &[])
            .await;

        // Success rate is 0.33, so no boost - should remain 0.8
        assert!((calibrated.calibrated_confidence - 0.8).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_calibrator_combined_adjustments() {
        let calibrator = ConfidenceCalibrator::new(CalibratorConfig::default());

        // Build high success rate
        for _ in 0..10 {
            calibrator.record_success("web_search", "search").await;
        }

        let signal = IntentSignal::with_tool("search", "web_search", 0.9);
        let recent_tools = vec!["web_search".to_string()];

        let calibrated = calibrator
            .calibrate_async(signal, RoutingLayer::L2Keyword, &recent_tools)
            .await;

        // L2 dampening: 0.9 * 0.9 = 0.81
        // History boost: 0.81 + min(1.0-0.81, 0.1) = 0.81 + 0.1 = 0.91
        // Context boost: 0.91 + 0.05 = 0.96
        assert!(calibrated.calibrated_confidence > 0.9);
    }
}
