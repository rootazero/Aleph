//! Confidence Calibration System
//!
//! Provides confidence calibration for intent signals:
//!
//! - Layer-specific calibration (L1/L2/L3)
//! - Tool-specific threshold overrides
//! - Context-based adjustments (recent tool usage)
//! - History-based boosting (successful patterns)

use crate::routing::{
    CalibratedSignal, CalibrationFactor, ConfidenceThresholds, IntentSignal, RoutingContext,
    RoutingLayerType, ToolConfidenceConfig,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// =============================================================================
// Calibration History
// =============================================================================

/// History of confidence calibrations for learning
#[derive(Debug, Default)]
pub struct CalibrationHistory {
    /// Success/failure counts per tool + pattern
    tool_patterns: HashMap<String, PatternStats>,

    /// Max entries to keep
    max_entries: usize,
}

#[derive(Debug, Clone)]
struct PatternStats {
    /// Total successes
    successes: u32,
    /// Total failures
    failures: u32,
    /// Last access timestamp
    last_access: std::time::Instant,
}

impl Default for PatternStats {
    fn default() -> Self {
        Self {
            successes: 0,
            failures: 0,
            last_access: std::time::Instant::now(),
        }
    }
}

impl CalibrationHistory {
    /// Create a new history with max entries
    pub fn new(max_entries: usize) -> Self {
        Self {
            tool_patterns: HashMap::new(),
            max_entries,
        }
    }

    /// Record a successful execution
    pub fn record_success(&mut self, tool_name: &str, input: &str) {
        let key = format!("{}:{}", tool_name, Self::normalize_input(input));
        let stats = self.tool_patterns.entry(key).or_default();
        stats.successes += 1;
        stats.last_access = std::time::Instant::now();
        self.prune_if_needed();
    }

    /// Record a failed/cancelled execution
    pub fn record_failure(&mut self, tool_name: &str, input: &str) {
        let key = format!("{}:{}", tool_name, Self::normalize_input(input));
        let stats = self.tool_patterns.entry(key).or_default();
        stats.failures += 1;
        stats.last_access = std::time::Instant::now();
        self.prune_if_needed();
    }

    /// Get success rate for a tool + input pattern
    pub fn get_success_rate(&self, tool_name: &str, input: &str) -> Option<f32> {
        let key = format!("{}:{}", tool_name, Self::normalize_input(input));
        self.tool_patterns.get(&key).map(|stats| {
            let total = stats.successes + stats.failures;
            if total == 0 {
                1.0
            } else {
                stats.successes as f32 / total as f32
            }
        })
    }

    /// Normalize input for pattern matching
    fn normalize_input(input: &str) -> String {
        // Simple normalization: lowercase, trim, first 50 chars
        let normalized = input.trim().to_lowercase();
        if normalized.len() > 50 {
            normalized[..50].to_string()
        } else {
            normalized
        }
    }

    /// Prune oldest entries if over capacity
    fn prune_if_needed(&mut self) {
        if self.tool_patterns.len() > self.max_entries {
            // Find oldest entry
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

    /// Clear all history
    pub fn clear(&mut self) {
        self.tool_patterns.clear();
    }
}

// =============================================================================
// Confidence Calibrator
// =============================================================================

/// Calibrates confidence scores based on various factors
pub struct ConfidenceCalibrator {
    /// Global confidence thresholds
    thresholds: ConfidenceThresholds,

    /// Tool-specific configurations
    tool_configs: HashMap<String, ToolConfidenceConfig>,

    /// Calibration history for learning
    history: Arc<RwLock<CalibrationHistory>>,

    /// L2 dampening factor for non-exact matches
    l2_dampening: f32,

    /// L3 model correction factor
    l3_correction: f32,

    /// Maximum context boost
    max_context_boost: f32,

    /// Boost per recent tool use
    recent_use_boost: f32,
}

impl ConfidenceCalibrator {
    /// Create a new calibrator with default settings
    pub fn new(thresholds: ConfidenceThresholds) -> Self {
        Self {
            thresholds,
            tool_configs: HashMap::new(),
            history: Arc::new(RwLock::new(CalibrationHistory::new(1000))),
            l2_dampening: 0.05,
            l3_correction: 0.1,
            max_context_boost: 0.15,
            recent_use_boost: 0.05,
        }
    }

    /// Create with tool-specific configurations
    pub fn with_tool_configs(
        thresholds: ConfidenceThresholds,
        tool_configs: HashMap<String, ToolConfidenceConfig>,
    ) -> Self {
        Self {
            thresholds,
            tool_configs,
            history: Arc::new(RwLock::new(CalibrationHistory::new(1000))),
            l2_dampening: 0.05,
            l3_correction: 0.1,
            max_context_boost: 0.15,
            recent_use_boost: 0.05,
        }
    }

    /// Calibrate an intent signal
    pub fn calibrate(&self, signal: IntentSignal, ctx: &RoutingContext) -> CalibratedSignal {
        let raw = signal.confidence;
        let mut calibrated = raw;
        let mut factors = Vec::new();

        // 1. Layer-specific calibration
        if let Some(factor) = self.apply_layer_calibration(&signal, &mut calibrated) {
            factors.push(factor);
        }

        // 2. Tool-specific calibration
        if let Some(ref tool) = signal.tool {
            if let Some(factor) = self.apply_tool_calibration(&tool.name, &mut calibrated) {
                factors.push(factor);
            }
        }

        // 3. Context-based calibration (recent tool usage)
        if let Some(factor) = self.apply_context_calibration(ctx, &signal, &mut calibrated) {
            factors.push(factor);
        }

        // Note: History-based boost requires async access to history
        // This is handled separately via calibrate_async()

        // Clamp to [0, 1]
        calibrated = calibrated.clamp(0.0, 1.0);

        CalibratedSignal::new(signal, calibrated)
            .with_factors(factors)
    }

    /// Calibrate with async history access
    pub async fn calibrate_async(
        &self,
        signal: IntentSignal,
        ctx: &RoutingContext,
    ) -> CalibratedSignal {
        let raw = signal.confidence;
        let mut calibrated = raw;
        let mut factors = Vec::new();

        // 1. Layer-specific calibration
        if let Some(factor) = self.apply_layer_calibration(&signal, &mut calibrated) {
            factors.push(factor);
        }

        // 2. Tool-specific calibration
        if let Some(ref tool) = signal.tool {
            if let Some(factor) = self.apply_tool_calibration(&tool.name, &mut calibrated) {
                factors.push(factor);
            }
        }

        // 3. Context-based calibration
        if let Some(factor) = self.apply_context_calibration(ctx, &signal, &mut calibrated) {
            factors.push(factor);
        }

        // 4. History-based boost
        if let Some(ref tool) = signal.tool {
            if let Some(factor) = self.apply_history_boost(&tool.name, ctx, &mut calibrated).await {
                factors.push(factor);
            }
        }

        // Clamp to [0, 1]
        calibrated = calibrated.clamp(0.0, 1.0);

        CalibratedSignal::new(signal, calibrated)
            .with_factors(factors)
    }

    /// Apply layer-specific calibration
    fn apply_layer_calibration(
        &self,
        signal: &IntentSignal,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        match signal.layer {
            RoutingLayerType::L1Regex => {
                // L1 is already well-calibrated, no adjustment
                None
            }
            RoutingLayerType::L2Semantic => {
                // L2 keyword matching can be over-confident for partial matches
                if *confidence > 0.7 && *confidence < 0.95 {
                    let adjustment = -self.l2_dampening;
                    *confidence += adjustment;
                    Some(CalibrationFactor::new(
                        "l2_dampening",
                        adjustment,
                        "L2 semantic match dampening for non-exact match",
                    ))
                } else {
                    None
                }
            }
            RoutingLayerType::L3Inference => {
                // L3 AI models can be overconfident
                let adjustment = -self.l3_correction;
                *confidence += adjustment;
                Some(CalibrationFactor::new(
                    "l3_model_correction",
                    adjustment,
                    "L3 AI model confidence correction",
                ))
            }
            RoutingLayerType::Default => None,
        }
    }

    /// Apply tool-specific calibration
    fn apply_tool_calibration(
        &self,
        tool_name: &str,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        if let Some(config) = self.tool_configs.get(tool_name) {
            // Apply minimum threshold
            if *confidence < config.min_threshold {
                // Don't boost, just mark as below threshold
                return Some(CalibrationFactor::new(
                    "below_min_threshold",
                    0.0,
                    format!(
                        "Confidence {} below tool minimum {}",
                        *confidence, config.min_threshold
                    ),
                ));
            }
        }
        None
    }

    /// Apply context-based calibration (recent tool usage)
    fn apply_context_calibration(
        &self,
        ctx: &RoutingContext,
        signal: &IntentSignal,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        // Check if tool was used recently in conversation
        if let Some(ref _conv) = ctx.conversation {
            if let Some(ref tool) = signal.tool {
                // Check conversation history for recent tool uses
                // Note: This requires ConversationContext to have recent_tool_uses method
                // For now, we'll use entity_hints as a proxy
                let tool_mentioned = ctx
                    .entity_hints
                    .iter()
                    .any(|h| h.to_lowercase().contains(&tool.name.to_lowercase()));

                if tool_mentioned {
                    let boost = self.recent_use_boost.min(self.max_context_boost);
                    *confidence += boost;
                    return Some(CalibrationFactor::new(
                        "context_boost",
                        boost,
                        format!("Tool '{}' referenced in conversation context", tool.name),
                    ));
                }
            }
        }
        None
    }

    /// Apply history-based boost (async)
    async fn apply_history_boost(
        &self,
        tool_name: &str,
        ctx: &RoutingContext,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        let history = self.history.read().await;

        if let Some(success_rate) = history.get_success_rate(tool_name, &ctx.input) {
            if success_rate > 0.8 {
                let boost = 0.1 * success_rate;
                *confidence += boost;
                return Some(CalibrationFactor::new(
                    "history_boost",
                    boost,
                    format!("Historical success rate: {:.0}%", success_rate * 100.0),
                ));
            }
        }

        None
    }

    /// Record a successful execution for learning
    pub async fn record_success(&self, tool_name: &str, input: &str) {
        let mut history = self.history.write().await;
        history.record_success(tool_name, input);
    }

    /// Record a failed/cancelled execution for learning
    pub async fn record_failure(&self, tool_name: &str, input: &str) {
        let mut history = self.history.write().await;
        history.record_failure(tool_name, input);
    }

    /// Get current thresholds
    pub fn thresholds(&self) -> &ConfidenceThresholds {
        &self.thresholds
    }

    /// Update thresholds
    pub fn set_thresholds(&mut self, thresholds: ConfidenceThresholds) {
        self.thresholds = thresholds;
    }

    /// Add or update a tool configuration
    pub fn set_tool_config(&mut self, tool_name: impl Into<String>, config: ToolConfidenceConfig) {
        self.tool_configs.insert(tool_name.into(), config);
    }

    /// Get tool configuration
    pub fn get_tool_config(&self, tool_name: &str) -> Option<&ToolConfidenceConfig> {
        self.tool_configs.get(tool_name)
    }

    /// Clear calibration history
    pub async fn clear_history(&self) {
        let mut history = self.history.write().await;
        history.clear();
    }
}

// Extend CalibratedSignal with helper method
impl CalibratedSignal {
    /// Add multiple calibration factors
    pub fn with_factors(mut self, factors: Vec<CalibrationFactor>) -> Self {
        self.calibration_factors = factors;
        self
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use crate::dispatcher::UnifiedTool;

    fn create_test_tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(name, name, &format!("{} tool", name), ToolSource::Native)
    }

    fn create_test_context() -> RoutingContext {
        RoutingContext::new("test input")
    }

    #[test]
    fn test_calibrator_creation() {
        let thresholds = ConfidenceThresholds::default();
        let calibrator = ConfidenceCalibrator::new(thresholds.clone());

        assert_eq!(calibrator.thresholds().no_match, thresholds.no_match);
    }

    #[test]
    fn test_l1_no_calibration() {
        let calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 1.0);
        let ctx = create_test_context();

        let calibrated = calibrator.calibrate(signal, &ctx);

        // L1 should not be adjusted
        assert!((calibrated.calibrated_confidence - 1.0).abs() < 0.01);
        assert!(calibrated.calibration_factors.is_empty());
    }

    #[test]
    fn test_l2_dampening() {
        let calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");
        let signal = IntentSignal::with_tool(RoutingLayerType::L2Semantic, tool, 0.85);
        let ctx = create_test_context();

        let calibrated = calibrator.calibrate(signal, &ctx);

        // L2 should be dampened
        assert!(calibrated.calibrated_confidence < 0.85);
        assert!(!calibrated.calibration_factors.is_empty());
        assert_eq!(calibrated.calibration_factors[0].name, "l2_dampening");
    }

    #[test]
    fn test_l3_correction() {
        let calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");
        let signal = IntentSignal::with_tool(RoutingLayerType::L3Inference, tool, 0.9);
        let ctx = create_test_context();

        let calibrated = calibrator.calibrate(signal, &ctx);

        // L3 should be corrected
        assert!(calibrated.calibrated_confidence < 0.9);
        assert!(!calibrated.calibration_factors.is_empty());
        assert_eq!(calibrated.calibration_factors[0].name, "l3_model_correction");
    }

    #[test]
    fn test_tool_config() {
        let mut calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());
        calibrator.set_tool_config("search", ToolConfidenceConfig::strict());

        let config = calibrator.get_tool_config("search");
        assert!(config.is_some());
        assert_eq!(config.unwrap().min_threshold, 0.6);
    }

    #[test]
    fn test_clamping() {
        let calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");

        // Test clamping at upper bound
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool.clone(), 1.5);
        let ctx = create_test_context();
        let calibrated = calibrator.calibrate(signal, &ctx);
        assert!(calibrated.calibrated_confidence <= 1.0);

        // Test clamping at lower bound
        let signal = IntentSignal::with_tool(RoutingLayerType::L3Inference, tool, -0.1);
        let calibrated = calibrator.calibrate(signal, &ctx);
        assert!(calibrated.calibrated_confidence >= 0.0);
    }

    #[tokio::test]
    async fn test_history_recording() {
        let calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());

        // Record some successes
        calibrator.record_success("search", "test query").await;
        calibrator.record_success("search", "test query").await;
        calibrator.record_failure("search", "test query").await;

        let history = calibrator.history.read().await;
        let rate = history.get_success_rate("search", "test query");
        assert!(rate.is_some());
        assert!((rate.unwrap() - 0.666).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_calibrate_async() {
        let calibrator = ConfidenceCalibrator::new(ConfidenceThresholds::default());

        // Record success to build history
        for _ in 0..5 {
            calibrator.record_success("search", "test query").await;
        }

        let tool = create_test_tool("search");
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.8);
        let ctx = RoutingContext::new("test query");

        let calibrated = calibrator.calibrate_async(signal, &ctx).await;

        // Should have history boost applied
        assert!(calibrated.calibrated_confidence > 0.8);
    }

    #[test]
    fn test_calibration_history() {
        let mut history = CalibrationHistory::new(10);

        history.record_success("tool1", "input1");
        history.record_success("tool1", "input1");
        history.record_failure("tool1", "input1");

        let rate = history.get_success_rate("tool1", "input1");
        assert!(rate.is_some());
        assert!((rate.unwrap() - 0.666).abs() < 0.01);

        // Non-existent entry
        let rate = history.get_success_rate("tool2", "input2");
        assert!(rate.is_none());
    }

    #[test]
    fn test_calibration_history_pruning() {
        let mut history = CalibrationHistory::new(3);

        // Add more entries than max
        history.record_success("tool1", "input1");
        history.record_success("tool2", "input2");
        history.record_success("tool3", "input3");
        history.record_success("tool4", "input4");

        // Should have pruned to max size
        assert!(history.tool_patterns.len() <= 3);
    }
}
