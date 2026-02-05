//! Structured Thinking Types for CoT Transparency
//!
//! This module provides types for representing and parsing the AI's
//! chain of thought reasoning, making the thinking process visible
//! and understandable to users.
//!
//! # Architecture
//!
//! ```text
//! LLM Response
//!     │
//!     ▼
//! ┌───────────────────┐
//! │  ThinkingParser   │  ← Parse reasoning text
//! └───────────────────┘
//!     │
//!     ▼
//! ┌───────────────────────────────────────────────┐
//! │           StructuredThinking                   │
//! │  ┌─────────────────────────────────────────┐  │
//! │  │ steps: Vec<ReasoningStep>                │  │
//! │  │   • Observation                          │  │
//! │  │   • Analysis                             │  │
//! │  │   • Planning                             │  │
//! │  │   • Decision                             │  │
//! │  └─────────────────────────────────────────┘  │
//! │  confidence: ConfidenceLevel                   │
//! │  alternatives_considered: Vec<String>          │
//! │  uncertainties: Vec<String>                    │
//! └───────────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};

/// Structured representation of AI's thinking process
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StructuredThinking {
    /// Raw reasoning text (backward compatible with existing reasoning field)
    pub reasoning: String,

    /// Parsed reasoning steps (semantic breakdown)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<ReasoningStep>>,

    /// Overall confidence in the decision
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ConfidenceLevel>,

    /// Alternative approaches that were considered
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives_considered: Vec<String>,

    /// Explicit uncertainties or knowledge gaps
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncertainties: Vec<String>,

    /// Duration of thinking phase in milliseconds (if extended thinking used)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_duration_ms: Option<u64>,
}

/// A single reasoning step with semantic type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    /// Step label (e.g., "Understanding the problem")
    pub label: String,

    /// Step content
    pub content: String,

    /// Semantic type for UI rendering
    pub step_type: ReasoningStepType,

    /// Substeps if this is a complex reasoning phase
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub substeps: Vec<String>,
}

/// Semantic type of a reasoning step
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStepType {
    /// Observing/understanding the current state
    Observation,
    /// Analyzing options, data, or trade-offs
    Analysis,
    /// Formulating a plan or approach
    Planning,
    /// Making the final decision
    Decision,
    /// Self-reflection, doubt, or reconsideration
    Reflection,
    /// Identifying risks or potential issues
    RiskAssessment,
    /// General thinking step
    #[default]
    General,
}

/// Confidence level for a decision
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConfidenceLevel {
    /// Very certain, strong evidence
    High,
    /// Reasonably confident
    #[default]
    Medium,
    /// Some uncertainty, proceed with caution
    Low,
    /// Exploratory, experimental approach
    Exploratory,
}

// ============================================================================
// Helper Methods
// ============================================================================

impl ReasoningStepType {
    /// Get the default label for this step type
    pub fn default_label(&self) -> &'static str {
        match self {
            Self::Observation => "Observation",
            Self::Analysis => "Analysis",
            Self::Planning => "Planning",
            Self::Decision => "Decision",
            Self::Reflection => "Reflection",
            Self::RiskAssessment => "Risk Assessment",
            Self::General => "Thinking",
        }
    }

    /// Get emoji for UI display
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Observation => "👁️",
            Self::Analysis => "🔍",
            Self::Planning => "📝",
            Self::Decision => "✅",
            Self::Reflection => "💭",
            Self::RiskAssessment => "⚠️",
            Self::General => "💡",
        }
    }

    /// All step types for iteration
    pub const ALL: &'static [ReasoningStepType] = &[
        Self::Observation,
        Self::Analysis,
        Self::Planning,
        Self::Decision,
        Self::Reflection,
        Self::RiskAssessment,
        Self::General,
    ];
}

impl ConfidenceLevel {
    /// Get emoji for UI display
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::High => "✅",
            Self::Medium => "🔵",
            Self::Low => "🟡",
            Self::Exploratory => "🔬",
        }
    }

    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::High => "High confidence",
            Self::Medium => "Moderate confidence",
            Self::Low => "Low confidence",
            Self::Exploratory => "Exploratory approach",
        }
    }
}

impl StructuredThinking {
    /// Create a new StructuredThinking from just reasoning text
    pub fn from_reasoning(reasoning: String) -> Self {
        Self {
            reasoning,
            ..Default::default()
        }
    }

    /// Check if structured steps are available
    pub fn has_steps(&self) -> bool {
        self.steps.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    /// Get the step count
    pub fn step_count(&self) -> usize {
        self.steps.as_ref().map(|s| s.len()).unwrap_or(0)
    }

    /// Check if there are any uncertainties
    pub fn has_uncertainties(&self) -> bool {
        !self.uncertainties.is_empty()
    }
}

impl ReasoningStep {
    /// Create a new reasoning step
    pub fn new(
        label: impl Into<String>,
        content: impl Into<String>,
        step_type: ReasoningStepType,
    ) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
            step_type,
            substeps: Vec::new(),
        }
    }

    /// Add a substep
    pub fn with_substep(mut self, substep: impl Into<String>) -> Self {
        self.substeps.push(substep.into());
        self
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_structured_thinking_default() {
        let thinking = StructuredThinking::default();
        assert!(thinking.reasoning.is_empty());
        assert!(thinking.steps.is_none());
        assert!(thinking.confidence.is_none());
        assert!(!thinking.has_steps());
    }

    #[test]
    fn test_from_reasoning() {
        let thinking = StructuredThinking::from_reasoning("I need to analyze this.".to_string());
        assert_eq!(thinking.reasoning, "I need to analyze this.");
        assert!(!thinking.has_steps());
    }

    #[test]
    fn test_reasoning_step_creation() {
        let step = ReasoningStep::new(
            "Understanding",
            "The user wants to implement caching.",
            ReasoningStepType::Observation,
        );

        assert_eq!(step.label, "Understanding");
        assert!(step.content.contains("caching"));
        assert_eq!(step.step_type, ReasoningStepType::Observation);
        assert!(step.substeps.is_empty());
    }

    #[test]
    fn test_reasoning_step_with_substeps() {
        let step = ReasoningStep::new("Analysis", "Comparing options", ReasoningStepType::Analysis)
            .with_substep("Option A: Redis")
            .with_substep("Option B: In-memory");

        assert_eq!(step.substeps.len(), 2);
    }

    #[test]
    fn test_step_type_emoji() {
        assert_eq!(ReasoningStepType::Observation.emoji(), "👁️");
        assert_eq!(ReasoningStepType::Analysis.emoji(), "🔍");
        assert_eq!(ReasoningStepType::Decision.emoji(), "✅");
    }

    #[test]
    fn test_step_type_default_label() {
        assert_eq!(
            ReasoningStepType::Observation.default_label(),
            "Observation"
        );
        assert_eq!(
            ReasoningStepType::RiskAssessment.default_label(),
            "Risk Assessment"
        );
    }

    #[test]
    fn test_confidence_level_emoji() {
        assert_eq!(ConfidenceLevel::High.emoji(), "✅");
        assert_eq!(ConfidenceLevel::Low.emoji(), "🟡");
        assert_eq!(ConfidenceLevel::Exploratory.emoji(), "🔬");
    }

    #[test]
    fn test_structured_thinking_with_steps() {
        let thinking = StructuredThinking {
            reasoning: "Full reasoning text".to_string(),
            steps: Some(vec![
                ReasoningStep::new("Observe", "Looking at the code", ReasoningStepType::Observation),
                ReasoningStep::new("Decide", "Will implement X", ReasoningStepType::Decision),
            ]),
            confidence: Some(ConfidenceLevel::High),
            ..Default::default()
        };

        assert!(thinking.has_steps());
        assert_eq!(thinking.step_count(), 2);
        assert_eq!(thinking.confidence, Some(ConfidenceLevel::High));
    }

    #[test]
    fn test_serde_roundtrip() {
        let thinking = StructuredThinking {
            reasoning: "Test reasoning".to_string(),
            steps: Some(vec![ReasoningStep::new(
                "Step 1",
                "Content",
                ReasoningStepType::Analysis,
            )]),
            confidence: Some(ConfidenceLevel::Medium),
            alternatives_considered: vec!["Alt A".to_string()],
            uncertainties: vec!["Not sure about X".to_string()],
            thinking_duration_ms: Some(150),
        };

        let json = serde_json::to_string(&thinking).unwrap();
        let deserialized: StructuredThinking = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.reasoning, thinking.reasoning);
        assert_eq!(deserialized.step_count(), 1);
        assert!(deserialized.has_uncertainties());
        assert_eq!(deserialized.thinking_duration_ms, Some(150));
    }

    #[test]
    fn test_has_uncertainties() {
        let mut thinking = StructuredThinking::default();
        assert!(!thinking.has_uncertainties());

        thinking.uncertainties.push("I'm not sure".to_string());
        assert!(thinking.has_uncertainties());
    }

    #[test]
    fn test_step_type_all_constant() {
        assert_eq!(ReasoningStepType::ALL.len(), 7);
        assert!(ReasoningStepType::ALL.contains(&ReasoningStepType::Observation));
        assert!(ReasoningStepType::ALL.contains(&ReasoningStepType::General));
    }

    #[test]
    fn test_confidence_level_description() {
        assert_eq!(ConfidenceLevel::High.description(), "High confidence");
        assert_eq!(ConfidenceLevel::Medium.description(), "Moderate confidence");
        assert_eq!(ConfidenceLevel::Low.description(), "Low confidence");
        assert_eq!(
            ConfidenceLevel::Exploratory.description(),
            "Exploratory approach"
        );
    }
}
