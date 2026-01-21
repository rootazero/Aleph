//! Intent Aggregator Module
//!
//! Combines signals from multiple routing layers into a single routing decision.
//!
//! # Overview
//!
//! The aggregator takes signals from L1 (regex), L2 (keyword), and L3 (AI)
//! routing layers and produces a unified `AggregatedIntent` with:
//!
//! - **Primary signal**: The highest confidence signal
//! - **Alternatives**: Other signals sorted by confidence
//! - **Action**: Recommended action based on confidence thresholds
//! - **Conflict detection**: When top signals are too close in confidence
//!
//! # Example
//!
//! ```ignore
//! use aethecore::intent::{IntentAggregator, AggregatorConfig, CalibratedSignal, RoutingLayer};
//!
//! let aggregator = IntentAggregator::new(AggregatorConfig::default());
//!
//! let signals = vec![
//!     CalibratedSignal::from_signal(&signal1, 0.9, RoutingLayer::L1Regex),
//!     CalibratedSignal::from_signal(&signal2, 0.75, RoutingLayer::L2Keyword),
//! ];
//!
//! let result = aggregator.aggregate(signals);
//! assert!(result.is_actionable());
//! ```

use super::calibrator::{CalibratedSignal, ConfidenceCalibrator, IntentSignal, RoutingLayer};
use serde::{Deserialize, Serialize};

// =============================================================================
// IntentAction
// =============================================================================

/// Recommended action based on aggregated intent confidence
#[derive(Debug, Clone, PartialEq)]
pub enum IntentAction {
    /// Execute immediately (high confidence >= 0.85)
    ExecuteImmediately,

    /// Confirm before executing (0.6 <= confidence < 0.85)
    ConfirmAndExecute,

    /// Request clarification (missing params or low confidence)
    RequestClarification {
        prompt: String,
        suggestions: Vec<String>,
    },

    /// Fall back to general chat (no specific intent)
    GeneralChat,
}

impl IntentAction {
    /// Check if this action allows execution (immediate or after confirm)
    pub fn allows_execution(&self) -> bool {
        matches!(
            self,
            IntentAction::ExecuteImmediately | IntentAction::ConfirmAndExecute
        )
    }

    /// Check if this action requires user input
    pub fn requires_input(&self) -> bool {
        matches!(
            self,
            IntentAction::ConfirmAndExecute | IntentAction::RequestClarification { .. }
        )
    }
}

// =============================================================================
// MissingParameter
// =============================================================================

/// Information about a missing required parameter
#[derive(Debug, Clone)]
pub struct MissingParameter {
    /// Parameter name
    pub name: String,

    /// Clarification prompt to ask user
    pub prompt: String,

    /// Suggested values for the parameter
    pub suggestions: Vec<String>,

    /// Whether this parameter is required
    pub is_required: bool,
}

impl MissingParameter {
    /// Create a new required missing parameter
    pub fn required(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            suggestions: Vec::new(),
            is_required: true,
        }
    }

    /// Create a new optional missing parameter
    pub fn optional(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            suggestions: Vec::new(),
            is_required: false,
        }
    }

    /// Add suggestions for this parameter
    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }
}

// =============================================================================
// AggregatedIntent
// =============================================================================

/// Result of aggregating multiple intent signals
#[derive(Debug, Clone)]
pub struct AggregatedIntent {
    /// The highest confidence signal (primary)
    pub primary_signal: CalibratedSignal,

    /// Alternative signals sorted by confidence
    pub alternatives: Vec<CalibratedSignal>,

    /// Final calibrated confidence (from primary signal)
    pub final_confidence: f32,

    /// Recommended action based on confidence and flags
    pub action: IntentAction,

    /// Whether top signals are in conflict (close confidence, different tools)
    pub has_conflict: bool,

    /// Missing required parameters
    pub missing_params: Vec<MissingParameter>,
}

impl AggregatedIntent {
    /// Create a new aggregated intent from a primary signal and action
    pub fn new(primary_signal: CalibratedSignal, action: IntentAction) -> Self {
        let final_confidence = primary_signal.calibrated_confidence;
        Self {
            primary_signal,
            alternatives: Vec::new(),
            final_confidence,
            action,
            has_conflict: false,
            missing_params: Vec::new(),
        }
    }

    /// Create a general chat result (no specific intent detected)
    pub fn general_chat() -> Self {
        // Create a minimal signal for general chat
        let signal = IntentSignal::new("general_chat", 0.0);
        let calibrated = CalibratedSignal::from_signal(&signal, 0.0, RoutingLayer::L1Regex);

        Self {
            primary_signal: calibrated,
            alternatives: Vec::new(),
            final_confidence: 0.0,
            action: IntentAction::GeneralChat,
            has_conflict: false,
            missing_params: Vec::new(),
        }
    }

    /// Get the tool name from the primary signal
    pub fn tool_name(&self) -> Option<&str> {
        self.primary_signal.tool_name.as_deref()
    }

    /// Get the intent type from the primary signal
    pub fn intent_type(&self) -> &str {
        &self.primary_signal.intent_type
    }

    /// Check if the action allows execution (Execute or Confirm)
    pub fn is_actionable(&self) -> bool {
        self.action.allows_execution()
    }

    /// Add missing parameters
    pub fn with_missing_params(mut self, params: Vec<MissingParameter>) -> Self {
        self.missing_params = params;
        self
    }

    /// Get the routing layer of the primary signal
    pub fn routing_layer(&self) -> RoutingLayer {
        self.primary_signal.layer
    }

    /// Check if all required parameters are present
    pub fn has_required_params(&self) -> bool {
        !self.missing_params.iter().any(|p| p.is_required)
    }
}

// =============================================================================
// AggregatorConfig
// =============================================================================

/// Configuration for the intent aggregator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatorConfig {
    /// Confidence threshold for immediate execution (default: 0.85)
    #[serde(default = "default_execute_threshold")]
    pub execute_threshold: f32,

    /// Confidence threshold for confirmation (default: 0.6)
    #[serde(default = "default_confirm_threshold")]
    pub confirm_threshold: f32,

    /// Confidence difference for conflict detection (default: 0.1)
    #[serde(default = "default_conflict_threshold")]
    pub conflict_threshold: f32,
}

fn default_execute_threshold() -> f32 {
    0.85
}

fn default_confirm_threshold() -> f32 {
    0.6
}

fn default_conflict_threshold() -> f32 {
    0.1
}

impl Default for AggregatorConfig {
    fn default() -> Self {
        Self {
            execute_threshold: default_execute_threshold(),
            confirm_threshold: default_confirm_threshold(),
            conflict_threshold: default_conflict_threshold(),
        }
    }
}

// =============================================================================
// IntentAggregator
// =============================================================================

/// Aggregates multiple intent signals into a single routing decision
///
/// The aggregator combines signals from different routing layers (L1, L2, L3)
/// and produces a unified result with action recommendations.
pub struct IntentAggregator {
    /// Configuration for thresholds
    config: AggregatorConfig,

    /// Optional calibrator for additional confidence adjustments
    calibrator: Option<ConfidenceCalibrator>,
}

impl IntentAggregator {
    /// Create a new aggregator with the given configuration
    pub fn new(config: AggregatorConfig) -> Self {
        Self {
            config,
            calibrator: None,
        }
    }

    /// Add a calibrator for additional confidence adjustments
    pub fn with_calibrator(mut self, calibrator: ConfidenceCalibrator) -> Self {
        self.calibrator = Some(calibrator);
        self
    }

    /// Get the configuration
    pub fn config(&self) -> &AggregatorConfig {
        &self.config
    }

    /// Aggregate multiple signals into a single intent
    ///
    /// # Arguments
    ///
    /// * `signals` - Vector of calibrated signals from routing layers
    ///
    /// # Returns
    ///
    /// `AggregatedIntent` with primary signal, alternatives, and recommended action
    pub fn aggregate(&self, mut signals: Vec<CalibratedSignal>) -> AggregatedIntent {
        // No signals = general chat
        if signals.is_empty() {
            return AggregatedIntent::general_chat();
        }

        // Sort by calibrated confidence (descending)
        signals.sort_by(|a, b| {
            b.calibrated_confidence
                .partial_cmp(&a.calibrated_confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Extract primary signal
        let primary_signal = signals.remove(0);
        let final_confidence = primary_signal.calibrated_confidence;

        // Detect conflicts with remaining signals
        let has_conflict = self.detect_conflict(&primary_signal, &signals);

        // Determine action based on confidence and conflict
        let action = self.determine_action(final_confidence, has_conflict);

        AggregatedIntent {
            primary_signal,
            alternatives: signals,
            final_confidence,
            action,
            has_conflict,
            missing_params: Vec::new(),
        }
    }

    /// Create an aggregated intent from a single signal
    pub fn from_single(&self, signal: CalibratedSignal) -> AggregatedIntent {
        let confidence = signal.calibrated_confidence;
        let action = self.determine_action(confidence, false);

        AggregatedIntent::new(signal, action)
    }

    /// Determine the recommended action based on confidence and flags
    pub fn determine_action(&self, confidence: f32, has_conflict: bool) -> IntentAction {
        // High confidence without conflict -> execute immediately
        if confidence >= self.config.execute_threshold && !has_conflict {
            return IntentAction::ExecuteImmediately;
        }

        // High confidence with conflict -> demote to confirm
        if confidence >= self.config.execute_threshold && has_conflict {
            return IntentAction::ConfirmAndExecute;
        }

        // Medium confidence -> confirm
        if confidence >= self.config.confirm_threshold {
            return IntentAction::ConfirmAndExecute;
        }

        // Low confidence -> general chat
        IntentAction::GeneralChat
    }

    /// Detect conflicts between primary and alternative signals
    ///
    /// A conflict exists when:
    /// 1. There are alternative signals
    /// 2. The top alternative has a different tool
    /// 3. The confidence difference is less than conflict_threshold
    fn detect_conflict(
        &self,
        primary: &CalibratedSignal,
        alternatives: &[CalibratedSignal],
    ) -> bool {
        if alternatives.is_empty() {
            return false;
        }

        let top_alt = &alternatives[0];

        // Check if different tools
        let primary_tool = primary.tool_name.as_ref();
        let alt_tool = top_alt.tool_name.as_ref();

        if primary_tool != alt_tool {
            // Check confidence difference
            let diff = (primary.calibrated_confidence - top_alt.calibrated_confidence).abs();
            if diff < self.config.conflict_threshold {
                return true;
            }
        }

        false
    }

    /// Override action to request clarification for missing parameters
    pub fn override_for_missing_params(
        &self,
        mut intent: AggregatedIntent,
        missing_params: Vec<MissingParameter>,
    ) -> AggregatedIntent {
        if missing_params.is_empty() {
            return intent;
        }

        // Check if any required parameters are missing
        let has_required_missing = missing_params.iter().any(|p| p.is_required);

        if has_required_missing {
            // Override action to request clarification
            let first_missing = &missing_params[0];
            intent.action = IntentAction::RequestClarification {
                prompt: first_missing.prompt.clone(),
                suggestions: first_missing.suggestions.clone(),
            };
        }

        intent.missing_params = missing_params;
        intent
    }
}

impl Default for IntentAggregator {
    fn default() -> Self {
        Self::new(AggregatorConfig::default())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Helper functions
    // -------------------------------------------------------------------------

    fn make_signal(
        intent: &str,
        tool: Option<&str>,
        confidence: f32,
        layer: RoutingLayer,
    ) -> CalibratedSignal {
        let mut signal = IntentSignal::new(intent, confidence);
        if let Some(t) = tool {
            signal.tool_name = Some(t.to_string());
        }
        CalibratedSignal::from_signal(&signal, confidence, layer)
    }

    // -------------------------------------------------------------------------
    // IntentAction tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_intent_action_allows_execution() {
        assert!(IntentAction::ExecuteImmediately.allows_execution());
        assert!(IntentAction::ConfirmAndExecute.allows_execution());
        assert!(!IntentAction::GeneralChat.allows_execution());
        assert!(!IntentAction::RequestClarification {
            prompt: "test".to_string(),
            suggestions: vec![],
        }
        .allows_execution());
    }

    #[test]
    fn test_intent_action_requires_input() {
        assert!(!IntentAction::ExecuteImmediately.requires_input());
        assert!(IntentAction::ConfirmAndExecute.requires_input());
        assert!(!IntentAction::GeneralChat.requires_input());
        assert!(IntentAction::RequestClarification {
            prompt: "test".to_string(),
            suggestions: vec![],
        }
        .requires_input());
    }

    // -------------------------------------------------------------------------
    // MissingParameter tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_missing_parameter_required() {
        let param = MissingParameter::required("path", "Please specify the file path");
        assert_eq!(param.name, "path");
        assert!(param.is_required);
        assert!(param.suggestions.is_empty());
    }

    #[test]
    fn test_missing_parameter_optional() {
        let param = MissingParameter::optional("limit", "How many results?");
        assert_eq!(param.name, "limit");
        assert!(!param.is_required);
    }

    #[test]
    fn test_missing_parameter_with_suggestions() {
        let param = MissingParameter::required("format", "Choose format")
            .with_suggestions(vec!["json".to_string(), "xml".to_string()]);
        assert_eq!(param.suggestions.len(), 2);
        assert_eq!(param.suggestions[0], "json");
    }

    // -------------------------------------------------------------------------
    // AggregatedIntent tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_aggregated_intent_general_chat() {
        let intent = AggregatedIntent::general_chat();
        assert_eq!(intent.intent_type(), "general_chat");
        assert_eq!(intent.final_confidence, 0.0);
        assert!(!intent.is_actionable());
        assert!(matches!(intent.action, IntentAction::GeneralChat));
    }

    #[test]
    fn test_aggregated_intent_tool_name() {
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);
        let intent = AggregatedIntent::new(signal, IntentAction::ExecuteImmediately);

        assert_eq!(intent.tool_name(), Some("web_search"));
        assert_eq!(intent.intent_type(), "search");
    }

    #[test]
    fn test_aggregated_intent_is_actionable() {
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);

        let intent1 = AggregatedIntent::new(signal.clone(), IntentAction::ExecuteImmediately);
        assert!(intent1.is_actionable());

        let intent2 = AggregatedIntent::new(signal.clone(), IntentAction::ConfirmAndExecute);
        assert!(intent2.is_actionable());

        let intent3 = AggregatedIntent::new(signal, IntentAction::GeneralChat);
        assert!(!intent3.is_actionable());
    }

    #[test]
    fn test_aggregated_intent_has_required_params() {
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);
        let mut intent = AggregatedIntent::new(signal, IntentAction::ExecuteImmediately);

        // No missing params
        assert!(intent.has_required_params());

        // Add optional missing param
        intent
            .missing_params
            .push(MissingParameter::optional("limit", "How many?"));
        assert!(intent.has_required_params());

        // Add required missing param
        intent
            .missing_params
            .push(MissingParameter::required("query", "What to search?"));
        assert!(!intent.has_required_params());
    }

    #[test]
    fn test_aggregated_intent_routing_layer() {
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L2Keyword);
        let intent = AggregatedIntent::new(signal, IntentAction::ConfirmAndExecute);

        assert_eq!(intent.routing_layer(), RoutingLayer::L2Keyword);
    }

    // -------------------------------------------------------------------------
    // AggregatorConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_aggregator_config_default() {
        let config = AggregatorConfig::default();
        assert_eq!(config.execute_threshold, 0.85);
        assert_eq!(config.confirm_threshold, 0.6);
        assert_eq!(config.conflict_threshold, 0.1);
    }

    #[test]
    fn test_aggregator_config_serialize() {
        let config = AggregatorConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("execute_threshold"));
        assert!(json.contains("0.85"));
    }

    #[test]
    fn test_aggregator_config_deserialize() {
        let json = r#"{"execute_threshold": 0.9, "confirm_threshold": 0.7}"#;
        let config: AggregatorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.execute_threshold, 0.9);
        assert_eq!(config.confirm_threshold, 0.7);
        // Default should fill in
        assert_eq!(config.conflict_threshold, 0.1);
    }

    // -------------------------------------------------------------------------
    // IntentAggregator tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_aggregator_new() {
        let config = AggregatorConfig::default();
        let aggregator = IntentAggregator::new(config.clone());
        assert_eq!(
            aggregator.config().execute_threshold,
            config.execute_threshold
        );
    }

    #[test]
    fn test_aggregator_empty_signals() {
        let aggregator = IntentAggregator::default();
        let result = aggregator.aggregate(vec![]);

        assert_eq!(result.intent_type(), "general_chat");
        assert!(!result.is_actionable());
    }

    #[test]
    fn test_aggregator_single_high_confidence() {
        let aggregator = IntentAggregator::default();
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);

        let result = aggregator.aggregate(vec![signal]);

        assert_eq!(result.tool_name(), Some("web_search"));
        assert!(result.is_actionable());
        assert!(matches!(result.action, IntentAction::ExecuteImmediately));
        assert!(!result.has_conflict);
    }

    #[test]
    fn test_aggregator_single_medium_confidence() {
        let aggregator = IntentAggregator::default();
        let signal = make_signal("search", Some("web_search"), 0.75, RoutingLayer::L2Keyword);

        let result = aggregator.aggregate(vec![signal]);

        assert!(result.is_actionable());
        assert!(matches!(result.action, IntentAction::ConfirmAndExecute));
    }

    #[test]
    fn test_aggregator_single_low_confidence() {
        let aggregator = IntentAggregator::default();
        let signal = make_signal("search", Some("web_search"), 0.4, RoutingLayer::L3Ai);

        let result = aggregator.aggregate(vec![signal]);

        assert!(!result.is_actionable());
        assert!(matches!(result.action, IntentAction::GeneralChat));
    }

    #[test]
    fn test_aggregator_multiple_signals_sorted() {
        let aggregator = IntentAggregator::default();

        let signals = vec![
            make_signal("file", Some("file_read"), 0.7, RoutingLayer::L2Keyword),
            make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex),
            make_signal("code", Some("code_gen"), 0.5, RoutingLayer::L3Ai),
        ];

        let result = aggregator.aggregate(signals);

        // Primary should be highest confidence
        assert_eq!(result.tool_name(), Some("web_search"));
        assert_eq!(result.final_confidence, 0.9);

        // Alternatives should be sorted
        assert_eq!(result.alternatives.len(), 2);
        assert_eq!(
            result.alternatives[0].tool_name,
            Some("file_read".to_string())
        );
        assert_eq!(
            result.alternatives[1].tool_name,
            Some("code_gen".to_string())
        );
    }

    #[test]
    fn test_aggregator_conflict_detection() {
        let aggregator = IntentAggregator::default();

        // Two different tools with similar confidence
        let signals = vec![
            make_signal("search", Some("web_search"), 0.88, RoutingLayer::L1Regex),
            make_signal("file", Some("file_read"), 0.85, RoutingLayer::L2Keyword),
        ];

        let result = aggregator.aggregate(signals);

        // Should detect conflict (difference 0.03 < 0.1)
        assert!(result.has_conflict);
        // High confidence with conflict -> demote to confirm
        assert!(matches!(result.action, IntentAction::ConfirmAndExecute));
    }

    #[test]
    fn test_aggregator_no_conflict_same_tool() {
        let aggregator = IntentAggregator::default();

        // Same tool with similar confidence - not a conflict
        let signals = vec![
            make_signal("search", Some("web_search"), 0.88, RoutingLayer::L1Regex),
            make_signal("search", Some("web_search"), 0.85, RoutingLayer::L2Keyword),
        ];

        let result = aggregator.aggregate(signals);

        assert!(!result.has_conflict);
    }

    #[test]
    fn test_aggregator_no_conflict_large_difference() {
        let aggregator = IntentAggregator::default();

        // Different tools but large confidence gap - not a conflict
        let signals = vec![
            make_signal("search", Some("web_search"), 0.95, RoutingLayer::L1Regex),
            make_signal("file", Some("file_read"), 0.7, RoutingLayer::L2Keyword),
        ];

        let result = aggregator.aggregate(signals);

        assert!(!result.has_conflict);
        assert!(matches!(result.action, IntentAction::ExecuteImmediately));
    }

    #[test]
    fn test_aggregator_from_single() {
        let aggregator = IntentAggregator::default();
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);

        let result = aggregator.from_single(signal);

        assert_eq!(result.tool_name(), Some("web_search"));
        assert!(result.alternatives.is_empty());
        assert!(!result.has_conflict);
    }

    #[test]
    fn test_aggregator_determine_action_thresholds() {
        let aggregator = IntentAggregator::default();

        // Execute immediately (>= 0.85, no conflict)
        assert!(matches!(
            aggregator.determine_action(0.9, false),
            IntentAction::ExecuteImmediately
        ));
        assert!(matches!(
            aggregator.determine_action(0.85, false),
            IntentAction::ExecuteImmediately
        ));

        // Confirm (>= 0.85 with conflict)
        assert!(matches!(
            aggregator.determine_action(0.9, true),
            IntentAction::ConfirmAndExecute
        ));

        // Confirm (0.6 <= confidence < 0.85)
        assert!(matches!(
            aggregator.determine_action(0.75, false),
            IntentAction::ConfirmAndExecute
        ));
        assert!(matches!(
            aggregator.determine_action(0.6, false),
            IntentAction::ConfirmAndExecute
        ));

        // General chat (< 0.6)
        assert!(matches!(
            aggregator.determine_action(0.5, false),
            IntentAction::GeneralChat
        ));
    }

    #[test]
    fn test_aggregator_override_for_missing_params() {
        let aggregator = IntentAggregator::default();
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);
        let intent = aggregator.from_single(signal);

        // No missing params - action unchanged
        let result = aggregator.override_for_missing_params(intent.clone(), vec![]);
        assert!(matches!(result.action, IntentAction::ExecuteImmediately));

        // Required missing param - action overridden
        let missing = vec![MissingParameter::required("query", "What to search?")
            .with_suggestions(vec!["news".to_string(), "weather".to_string()])];

        let result = aggregator.override_for_missing_params(intent.clone(), missing);
        match result.action {
            IntentAction::RequestClarification {
                prompt,
                suggestions,
            } => {
                assert_eq!(prompt, "What to search?");
                assert_eq!(suggestions.len(), 2);
            }
            _ => panic!("Expected RequestClarification action"),
        }

        // Optional missing param - action unchanged
        let missing = vec![MissingParameter::optional("limit", "How many results?")];
        let result = aggregator.override_for_missing_params(intent, missing);
        assert!(matches!(result.action, IntentAction::ExecuteImmediately));
        assert_eq!(result.missing_params.len(), 1);
    }

    #[test]
    fn test_aggregator_custom_config() {
        let config = AggregatorConfig {
            execute_threshold: 0.95,
            confirm_threshold: 0.7,
            conflict_threshold: 0.05,
        };
        let aggregator = IntentAggregator::new(config);

        // 0.9 confidence now below execute threshold
        let signal = make_signal("search", Some("web_search"), 0.9, RoutingLayer::L1Regex);
        let result = aggregator.aggregate(vec![signal]);

        assert!(matches!(result.action, IntentAction::ConfirmAndExecute));

        // 0.65 confidence now below confirm threshold
        let signal = make_signal("search", Some("web_search"), 0.65, RoutingLayer::L2Keyword);
        let result = aggregator.aggregate(vec![signal]);

        assert!(matches!(result.action, IntentAction::GeneralChat));
    }

    #[test]
    fn test_aggregator_edge_case_no_tool_name() {
        let aggregator = IntentAggregator::default();

        // Signals without tool names
        let signals = vec![
            make_signal("intent1", None, 0.88, RoutingLayer::L1Regex),
            make_signal("intent2", None, 0.85, RoutingLayer::L2Keyword),
        ];

        let result = aggregator.aggregate(signals);

        // No tool names = no conflict (both are None, so they're "equal")
        assert!(!result.has_conflict);
    }

    #[test]
    fn test_aggregator_conflict_threshold_boundary() {
        // Test conflict threshold behavior
        // Conflict is detected when diff < threshold (not <=)
        let config = AggregatorConfig {
            conflict_threshold: 0.1,
            ..Default::default()
        };
        let aggregator = IntentAggregator::new(config);

        // Large gap (diff = 0.15) - clearly not a conflict
        let signals = vec![
            make_signal("search", Some("web_search"), 0.95, RoutingLayer::L1Regex),
            make_signal("file", Some("file_read"), 0.80, RoutingLayer::L2Keyword),
        ];
        let result = aggregator.aggregate(signals);
        assert!(!result.has_conflict, "Diff of 0.15 should NOT be conflict");

        // Small gap (diff = 0.05) - clearly is a conflict
        let signals = vec![
            make_signal("search", Some("web_search"), 0.85, RoutingLayer::L1Regex),
            make_signal("file", Some("file_read"), 0.80, RoutingLayer::L2Keyword),
        ];
        let result = aggregator.aggregate(signals);
        assert!(
            result.has_conflict,
            "Diff of 0.05 should be conflict (< 0.1)"
        );

        // Medium gap (diff = 0.12) - not a conflict
        let signals = vec![
            make_signal("search", Some("web_search"), 0.92, RoutingLayer::L1Regex),
            make_signal("file", Some("file_read"), 0.80, RoutingLayer::L2Keyword),
        ];
        let result = aggregator.aggregate(signals);
        assert!(
            !result.has_conflict,
            "Diff of 0.12 should NOT be conflict (> 0.1)"
        );
    }
}
