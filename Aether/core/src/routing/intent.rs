//! Intent Types for Routing Pipeline
//!
//! Core data structures for the enhanced intent routing pipeline:
//!
//! - `IntentSignal`: Signal from a single routing layer
//! - `AggregatedIntent`: Combined intent from all layers
//! - `IntentAction`: Recommended action based on confidence
//! - `ParameterRequirement`: Description of a missing parameter

use crate::dispatcher::UnifiedTool;
use crate::routing::RoutingLayerType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;

// =============================================================================
// Intent Signal
// =============================================================================

/// Signal from a single routing layer
///
/// Each layer (L1/L2/L3) produces an IntentSignal when it matches a tool.
/// These signals are later aggregated by the IntentAggregator.
#[derive(Debug, Clone)]
pub struct IntentSignal {
    /// Source layer that produced this signal
    pub layer: RoutingLayerType,

    /// Matched tool (if any)
    pub tool: Option<UnifiedTool>,

    /// Confidence score (0.0-1.0)
    pub confidence: f32,

    /// Extracted parameters
    pub parameters: Value,

    /// Reasoning for the match
    pub reason: String,

    /// Processing latency in milliseconds
    pub latency_ms: u64,

    /// Matched keywords (for L2 matches)
    pub matched_keywords: Vec<String>,

    /// Timestamp when signal was created
    pub created_at: Instant,
}

impl IntentSignal {
    /// Create a new intent signal
    pub fn new(layer: RoutingLayerType, confidence: f32) -> Self {
        Self {
            layer,
            tool: None,
            confidence,
            parameters: Value::Object(serde_json::Map::new()),
            reason: String::new(),
            latency_ms: 0,
            matched_keywords: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// Create a signal with a matched tool
    pub fn with_tool(layer: RoutingLayerType, tool: UnifiedTool, confidence: f32) -> Self {
        Self {
            layer,
            tool: Some(tool),
            confidence,
            parameters: Value::Object(serde_json::Map::new()),
            reason: String::new(),
            latency_ms: 0,
            matched_keywords: Vec::new(),
            created_at: Instant::now(),
        }
    }

    /// Builder: set parameters
    pub fn with_parameters(mut self, params: Value) -> Self {
        self.parameters = params;
        self
    }

    /// Builder: set reason
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    /// Builder: set latency
    pub fn with_latency(mut self, latency_ms: u64) -> Self {
        self.latency_ms = latency_ms;
        self
    }

    /// Builder: set matched keywords
    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.matched_keywords = keywords;
        self
    }

    /// Check if this signal has a tool match
    pub fn has_tool(&self) -> bool {
        self.tool.is_some()
    }

    /// Check if confidence meets the layer's threshold
    pub fn meets_threshold(&self) -> bool {
        self.confidence >= self.layer.min_confidence()
    }
}

// =============================================================================
// Aggregated Intent
// =============================================================================

/// Aggregated intent from all routing layers
///
/// This is the final output of the IntentAggregator, combining signals
/// from multiple layers into a single routing decision.
#[derive(Debug, Clone)]
pub struct AggregatedIntent {
    /// Primary intent signal (highest confidence after calibration)
    pub primary: IntentSignal,

    /// Alternative signals (for disambiguation UI if needed)
    pub alternatives: Vec<IntentSignal>,

    /// Final confidence after calibration and aggregation
    pub final_confidence: f32,

    /// Whether all required parameters are provided
    pub parameters_complete: bool,

    /// Missing parameters (for clarification)
    pub missing_parameters: Vec<ParameterRequirement>,

    /// Recommended action based on confidence and completeness
    pub action: IntentAction,

    /// Whether there's a conflict between layer signals
    pub has_conflict: bool,
}

impl AggregatedIntent {
    /// Create a new aggregated intent from a primary signal
    pub fn new(primary: IntentSignal, action: IntentAction) -> Self {
        let final_confidence = primary.confidence;
        Self {
            primary,
            alternatives: Vec::new(),
            final_confidence,
            parameters_complete: true,
            missing_parameters: Vec::new(),
            action,
            has_conflict: false,
        }
    }

    /// Create a GeneralChat intent (no tool matched)
    pub fn general_chat() -> Self {
        Self {
            primary: IntentSignal::new(RoutingLayerType::Default, 0.0),
            alternatives: Vec::new(),
            final_confidence: 0.0,
            parameters_complete: true,
            missing_parameters: Vec::new(),
            action: IntentAction::GeneralChat,
            has_conflict: false,
        }
    }

    /// Check if a tool was matched
    pub fn has_tool(&self) -> bool {
        self.primary.tool.is_some()
    }

    /// Get the matched tool name (if any)
    pub fn tool_name(&self) -> Option<&str> {
        self.primary.tool.as_ref().map(|t| t.name.as_str())
    }

    /// Check if this intent requires user interaction
    pub fn requires_interaction(&self) -> bool {
        matches!(
            self.action,
            IntentAction::RequestConfirmation | IntentAction::RequestClarification { .. }
        )
    }
}

// =============================================================================
// Intent Action
// =============================================================================

/// Recommended action based on confidence and parameter completeness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentAction {
    /// Execute tool directly (confidence >= auto_execute threshold)
    Execute,

    /// Request user confirmation (medium confidence)
    RequestConfirmation,

    /// Request clarification for missing parameters
    RequestClarification {
        /// Prompt to show the user
        prompt: String,
        /// Suggested values (if any)
        suggestions: Vec<String>,
    },

    /// Fall back to general chat (no tool match)
    GeneralChat,
}

impl IntentAction {
    /// Check if this action allows direct execution
    pub fn can_execute_directly(&self) -> bool {
        matches!(self, Self::Execute)
    }

    /// Check if this action requires user input
    pub fn requires_user_input(&self) -> bool {
        matches!(
            self,
            Self::RequestConfirmation | Self::RequestClarification { .. }
        )
    }
}

impl Default for IntentAction {
    fn default() -> Self {
        Self::GeneralChat
    }
}

// =============================================================================
// Parameter Requirement
// =============================================================================

/// Description of a required parameter that is missing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterRequirement {
    /// Parameter name
    pub name: String,

    /// Parameter type (string, number, boolean, etc.)
    pub param_type: String,

    /// Whether the parameter is required
    pub required: bool,

    /// Human-readable description
    pub description: String,

    /// Prompt to show when requesting this parameter
    pub clarification_prompt: String,

    /// Suggested values (if applicable)
    pub suggestions: Vec<String>,
}

impl ParameterRequirement {
    /// Create a new parameter requirement
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        let description = description.into();
        Self {
            clarification_prompt: format!("请提供 {}：", &description),
            name,
            param_type: "string".to_string(),
            required: true,
            description,
            suggestions: Vec::new(),
        }
    }

    /// Builder: set param type
    pub fn with_type(mut self, param_type: impl Into<String>) -> Self {
        self.param_type = param_type.into();
        self
    }

    /// Builder: set as optional
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Builder: set clarification prompt
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.clarification_prompt = prompt.into();
        self
    }

    /// Builder: set suggestions
    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }
}

// =============================================================================
// Calibrated Signal
// =============================================================================

/// An IntentSignal with calibration applied
#[derive(Debug, Clone)]
pub struct CalibratedSignal {
    /// Original signal
    pub signal: IntentSignal,

    /// Raw confidence before calibration
    pub raw_confidence: f32,

    /// Calibrated confidence
    pub calibrated_confidence: f32,

    /// Calibration factors applied
    pub calibration_factors: Vec<CalibrationFactor>,
}

impl CalibratedSignal {
    /// Create a new calibrated signal
    pub fn new(signal: IntentSignal, calibrated_confidence: f32) -> Self {
        let raw_confidence = signal.confidence;
        Self {
            signal,
            raw_confidence,
            calibrated_confidence,
            calibration_factors: Vec::new(),
        }
    }

    /// Builder: add a calibration factor
    pub fn with_factor(mut self, factor: CalibrationFactor) -> Self {
        self.calibration_factors.push(factor);
        self
    }

    /// Get total adjustment from all factors
    pub fn total_adjustment(&self) -> f32 {
        self.calibration_factors
            .iter()
            .map(|f| f.adjustment)
            .sum()
    }
}

// =============================================================================
// Calibration Factor
// =============================================================================

/// A single calibration adjustment factor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationFactor {
    /// Factor name (for debugging/logging)
    pub name: String,

    /// Adjustment value (-1.0 to 1.0)
    pub adjustment: f32,

    /// Reason for the adjustment
    pub reason: String,
}

impl CalibrationFactor {
    /// Create a new calibration factor
    pub fn new(
        name: impl Into<String>,
        adjustment: f32,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            adjustment,
            reason: reason.into(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;

    fn create_test_tool() -> UnifiedTool {
        UnifiedTool::new("search", "search", "Search the web", ToolSource::Native)
    }

    #[test]
    fn test_intent_signal_creation() {
        let signal = IntentSignal::new(RoutingLayerType::L1Regex, 1.0);
        assert_eq!(signal.confidence, 1.0);
        assert!(signal.tool.is_none());
        assert!(!signal.has_tool());
    }

    #[test]
    fn test_intent_signal_with_tool() {
        let tool = create_test_tool();
        let signal = IntentSignal::with_tool(RoutingLayerType::L2Semantic, tool.clone(), 0.8)
            .with_reason("Keyword match")
            .with_latency(150);

        assert!(signal.has_tool());
        assert_eq!(signal.tool.as_ref().unwrap().name, "search");
        assert_eq!(signal.confidence, 0.8);
        assert_eq!(signal.reason, "Keyword match");
        assert_eq!(signal.latency_ms, 150);
    }

    #[test]
    fn test_aggregated_intent_general_chat() {
        let intent = AggregatedIntent::general_chat();
        assert!(!intent.has_tool());
        assert_eq!(intent.final_confidence, 0.0);
        assert!(matches!(intent.action, IntentAction::GeneralChat));
    }

    #[test]
    fn test_intent_action_properties() {
        assert!(IntentAction::Execute.can_execute_directly());
        assert!(!IntentAction::GeneralChat.can_execute_directly());

        assert!(IntentAction::RequestConfirmation.requires_user_input());
        assert!(IntentAction::RequestClarification {
            prompt: "test".to_string(),
            suggestions: vec![]
        }
        .requires_user_input());
        assert!(!IntentAction::Execute.requires_user_input());
    }

    #[test]
    fn test_parameter_requirement() {
        let param = ParameterRequirement::new("location", "城市名称")
            .with_type("string")
            .with_suggestions(vec!["北京".to_string(), "上海".to_string()]);

        assert_eq!(param.name, "location");
        assert!(param.required);
        assert_eq!(param.suggestions.len(), 2);
        assert!(param.clarification_prompt.contains("城市名称"));
    }

    #[test]
    fn test_calibrated_signal() {
        let tool = create_test_tool();
        let signal = IntentSignal::with_tool(RoutingLayerType::L2Semantic, tool, 0.75);

        let calibrated = CalibratedSignal::new(signal, 0.70)
            .with_factor(CalibrationFactor::new(
                "l2_dampening",
                -0.05,
                "L2 dampening applied",
            ));

        assert_eq!(calibrated.raw_confidence, 0.75);
        assert_eq!(calibrated.calibrated_confidence, 0.70);
        assert_eq!(calibrated.total_adjustment(), -0.05);
    }
}
