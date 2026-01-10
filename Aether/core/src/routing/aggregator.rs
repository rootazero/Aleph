//! Intent Aggregator
//!
//! Aggregates signals from multiple routing layers into a single routing decision:
//!
//! - Signal sorting by calibrated confidence
//! - Conflict detection between layers
//! - Action determination based on thresholds
//! - Parameter completeness checking

use crate::routing::{
    AggregatedIntent, CalibratedSignal, ConfidenceCalibrator, ConfidenceThresholds, IntentAction,
    IntentSignal, ParameterRequirement, RoutingContext,
};
use serde_json::Value;
use tracing::debug;

/// Intent Aggregator
///
/// Combines signals from multiple routing layers into a single `AggregatedIntent`.
pub struct IntentAggregator {
    /// Confidence thresholds for action determination
    thresholds: ConfidenceThresholds,

    /// Calibrator for adjusting signal confidence
    calibrator: Option<ConfidenceCalibrator>,

    /// Conflict threshold - if top signals differ by less than this, mark as conflict
    conflict_threshold: f32,
}

impl IntentAggregator {
    /// Create a new aggregator with thresholds
    pub fn new(thresholds: ConfidenceThresholds) -> Self {
        Self {
            thresholds,
            calibrator: None,
            conflict_threshold: 0.1,
        }
    }

    /// Create with calibrator
    pub fn with_calibrator(mut self, calibrator: ConfidenceCalibrator) -> Self {
        self.calibrator = Some(calibrator);
        self
    }

    /// Set conflict threshold
    pub fn with_conflict_threshold(mut self, threshold: f32) -> Self {
        self.conflict_threshold = threshold;
        self
    }

    /// Aggregate multiple signals into a single intent
    ///
    /// # Arguments
    ///
    /// * `signals` - Signals from routing layers
    /// * `ctx` - Routing context
    ///
    /// # Returns
    ///
    /// `AggregatedIntent` with primary signal, alternatives, and recommended action
    pub async fn aggregate(
        &self,
        signals: Vec<IntentSignal>,
        ctx: &RoutingContext,
    ) -> AggregatedIntent {
        // No signals = general chat
        if signals.is_empty() {
            return AggregatedIntent::general_chat();
        }

        // Calibrate signals
        let mut calibrated: Vec<CalibratedSignal> = if let Some(ref calibrator) = self.calibrator {
            let mut result = Vec::with_capacity(signals.len());
            for signal in signals {
                let calibrated = calibrator.calibrate_async(signal, ctx).await;
                result.push(calibrated);
            }
            result
        } else {
            signals
                .into_iter()
                .map(|s| CalibratedSignal::new(s.clone(), s.confidence))
                .collect()
        };

        // Sort by calibrated confidence (descending)
        calibrated.sort_by(|a, b| {
            b.calibrated_confidence
                .partial_cmp(&a.calibrated_confidence)
                .unwrap()
        });

        // Extract primary and alternatives
        let primary_calibrated = calibrated.remove(0);
        let alternatives: Vec<IntentSignal> = calibrated
            .into_iter()
            .map(|c| c.signal)
            .collect();

        // Check for conflicts
        let has_conflict = self.detect_conflict(&primary_calibrated, &alternatives);

        // Determine action
        let action = self.determine_action(
            primary_calibrated.calibrated_confidence,
            has_conflict,
            &primary_calibrated.signal,
        );

        // Check parameter completeness
        let (parameters_complete, missing_parameters) =
            self.check_parameter_completeness(&primary_calibrated.signal);

        // Build aggregated intent
        let mut intent = AggregatedIntent::new(primary_calibrated.signal, action);
        intent.alternatives = alternatives;
        intent.final_confidence = primary_calibrated.calibrated_confidence;
        intent.has_conflict = has_conflict;
        intent.parameters_complete = parameters_complete;
        intent.missing_parameters = missing_parameters;

        // Override action if parameters missing
        if !intent.parameters_complete && !intent.missing_parameters.is_empty() {
            let first_missing = &intent.missing_parameters[0];
            intent.action = IntentAction::RequestClarification {
                prompt: first_missing.clarification_prompt.clone(),
                suggestions: first_missing.suggestions.clone(),
            };
        }

        debug!(
            tool = ?intent.tool_name(),
            confidence = intent.final_confidence,
            has_conflict,
            parameters_complete,
            action = ?intent.action,
            "IntentAggregator: Aggregated intent"
        );

        intent
    }

    /// Aggregate from a single signal
    pub fn from_single(&self, signal: IntentSignal) -> AggregatedIntent {
        let confidence = signal.confidence;
        let action = self.determine_action(confidence, false, &signal);

        let (parameters_complete, missing_parameters) = self.check_parameter_completeness(&signal);

        let mut intent = AggregatedIntent::new(signal, action);
        intent.parameters_complete = parameters_complete;
        intent.missing_parameters = missing_parameters;

        // Override action if parameters missing
        if !intent.parameters_complete && !intent.missing_parameters.is_empty() {
            let first_missing = &intent.missing_parameters[0];
            intent.action = IntentAction::RequestClarification {
                prompt: first_missing.clarification_prompt.clone(),
                suggestions: first_missing.suggestions.clone(),
            };
        }

        intent
    }

    /// Detect conflicts between top signals
    fn detect_conflict(
        &self,
        primary: &CalibratedSignal,
        alternatives: &[IntentSignal],
    ) -> bool {
        if alternatives.is_empty() {
            return false;
        }

        // Check if primary and next best have different tools but similar confidence
        let primary_tool = primary.signal.tool.as_ref().map(|t| &t.name);
        let alt_tool = alternatives[0].tool.as_ref().map(|t| &t.name);

        // Different tools
        if primary_tool != alt_tool {
            // Similar confidence?
            let confidence_diff = primary.calibrated_confidence - alternatives[0].confidence;
            if confidence_diff.abs() < self.conflict_threshold {
                return true;
            }
        }

        false
    }

    /// Determine action based on confidence and flags
    fn determine_action(
        &self,
        confidence: f32,
        has_conflict: bool,
        signal: &IntentSignal,
    ) -> IntentAction {
        // No tool matched
        if !signal.has_tool() || confidence < self.thresholds.no_match {
            return IntentAction::GeneralChat;
        }

        // High confidence and no conflict = execute
        if confidence >= self.thresholds.auto_execute && !has_conflict {
            return IntentAction::Execute;
        }

        // Medium confidence or has conflict = request confirmation
        if confidence >= self.thresholds.requires_confirmation || has_conflict {
            return IntentAction::RequestConfirmation;
        }

        // Low confidence = general chat
        IntentAction::GeneralChat
    }

    /// Check if all required parameters are provided
    fn check_parameter_completeness(
        &self,
        signal: &IntentSignal,
    ) -> (bool, Vec<ParameterRequirement>) {
        let tool = match &signal.tool {
            Some(t) => t,
            None => return (true, Vec::new()), // No tool = no params needed
        };

        // Parse tool schema for required parameters
        let required_params = self.get_required_params(&tool.parameters_schema);
        if required_params.is_empty() {
            return (true, Vec::new());
        }

        // Check which required params are missing
        let provided_params = &signal.parameters;
        let mut missing = Vec::new();

        for (name, description) in required_params {
            let has_param = match provided_params {
                Value::Object(map) => map.contains_key(&name),
                _ => false,
            };

            if !has_param {
                missing.push(ParameterRequirement::new(&name, &description));
            }
        }

        let complete = missing.is_empty();
        (complete, missing)
    }

    /// Parse required parameters from tool schema
    fn get_required_params(&self, schema: &Option<Value>) -> Vec<(String, String)> {
        let schema = match schema {
            Some(s) => s,
            None => return Vec::new(),
        };

        // Parse JSON Schema for required fields
        let mut result = Vec::new();

        // Get required array
        let required: Vec<&str> = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect()
            })
            .unwrap_or_default();

        // Get properties
        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            for (name, prop) in properties {
                if required.contains(&name.as_str()) {
                    let description = prop
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or(name)
                        .to_string();
                    result.push((name.clone(), description));
                }
            }
        }

        result
    }

    /// Get current thresholds
    pub fn thresholds(&self) -> &ConfidenceThresholds {
        &self.thresholds
    }

    /// Update thresholds
    pub fn set_thresholds(&mut self, thresholds: ConfidenceThresholds) {
        self.thresholds = thresholds;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::{ToolSource, UnifiedTool};
    use crate::routing::RoutingLayerType;

    fn create_test_tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(name, name, &format!("{} tool", name), ToolSource::Native)
    }

    fn create_test_tool_with_schema() -> UnifiedTool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词"
                },
                "location": {
                    "type": "string",
                    "description": "位置"
                }
            },
            "required": ["query", "location"]
        });

        UnifiedTool::new("search", "search", "Search tool", ToolSource::Native)
            .with_parameters_schema(schema)
    }

    #[test]
    fn test_aggregator_creation() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        assert_eq!(aggregator.conflict_threshold, 0.1);
    }

    #[tokio::test]
    async fn test_aggregate_single_signal() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.95);

        let ctx = RoutingContext::new("test input");
        let result = aggregator.aggregate(vec![signal], &ctx).await;

        assert!(result.has_tool());
        assert_eq!(result.tool_name(), Some("search"));
        assert!(matches!(result.action, IntentAction::Execute));
    }

    #[tokio::test]
    async fn test_aggregate_empty_signals() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        let ctx = RoutingContext::new("test input");

        let result = aggregator.aggregate(vec![], &ctx).await;

        assert!(!result.has_tool());
        assert!(matches!(result.action, IntentAction::GeneralChat));
    }

    #[tokio::test]
    async fn test_aggregate_multiple_signals() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());

        let signal1 = IntentSignal::with_tool(
            RoutingLayerType::L2Semantic,
            create_test_tool("search"),
            0.7,
        );
        let signal2 = IntentSignal::with_tool(
            RoutingLayerType::L3Inference,
            create_test_tool("translate"),
            0.6,
        );

        let ctx = RoutingContext::new("test input");
        let result = aggregator.aggregate(vec![signal1, signal2], &ctx).await;

        // Higher confidence signal should be primary
        assert_eq!(result.tool_name(), Some("search"));
        assert_eq!(result.alternatives.len(), 1);
    }

    #[tokio::test]
    async fn test_conflict_detection() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default())
            .with_conflict_threshold(0.15);

        // Two signals with similar confidence but different tools
        let signal1 = IntentSignal::with_tool(
            RoutingLayerType::L2Semantic,
            create_test_tool("search"),
            0.72,
        );
        let signal2 = IntentSignal::with_tool(
            RoutingLayerType::L3Inference,
            create_test_tool("translate"),
            0.68,
        );

        let ctx = RoutingContext::new("test input");
        let result = aggregator.aggregate(vec![signal1, signal2], &ctx).await;

        // Should detect conflict due to similar confidence
        assert!(result.has_conflict);
        // With conflict, should request confirmation
        assert!(matches!(result.action, IntentAction::RequestConfirmation));
    }

    #[test]
    fn test_from_single() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 1.0);

        let result = aggregator.from_single(signal);

        assert!(result.has_tool());
        assert_eq!(result.final_confidence, 1.0);
        assert!(matches!(result.action, IntentAction::Execute));
    }

    #[test]
    fn test_action_determination() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        let tool = create_test_tool("search");

        // High confidence
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool.clone(), 0.95);
        let action = aggregator.determine_action(0.95, false, &signal);
        assert!(matches!(action, IntentAction::Execute));

        // Medium confidence
        let signal = IntentSignal::with_tool(RoutingLayerType::L2Semantic, tool.clone(), 0.75);
        let action = aggregator.determine_action(0.75, false, &signal);
        assert!(matches!(action, IntentAction::RequestConfirmation));

        // Low confidence
        let signal = IntentSignal::with_tool(RoutingLayerType::L3Inference, tool.clone(), 0.2);
        let action = aggregator.determine_action(0.2, false, &signal);
        assert!(matches!(action, IntentAction::GeneralChat));

        // High confidence but conflict
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.95);
        let action = aggregator.determine_action(0.95, true, &signal);
        assert!(matches!(action, IntentAction::RequestConfirmation));
    }

    #[test]
    fn test_parameter_completeness() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        let tool = create_test_tool_with_schema();

        // Missing parameters
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool.clone(), 0.95)
            .with_parameters(serde_json::json!({"query": "weather"}));

        let (complete, missing) = aggregator.check_parameter_completeness(&signal);
        assert!(!complete);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "location");

        // All parameters provided
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.95)
            .with_parameters(serde_json::json!({"query": "weather", "location": "Beijing"}));

        let (complete, missing) = aggregator.check_parameter_completeness(&signal);
        assert!(complete);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_missing_params_trigger_clarification() {
        let aggregator = IntentAggregator::new(ConfidenceThresholds::default());
        let tool = create_test_tool_with_schema();

        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.95)
            .with_parameters(serde_json::json!({"query": "weather"}));

        let result = aggregator.from_single(signal);

        // Should override to clarification
        assert!(matches!(
            result.action,
            IntentAction::RequestClarification { .. }
        ));
    }
}
