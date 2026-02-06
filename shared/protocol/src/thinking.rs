//! Reasoning and Confidence Types
//!
//! Types for structured reasoning blocks and confidence levels in agent thinking.

use serde::{Deserialize, Serialize};

/// Semantic type of a reasoning step.
///
/// Used to categorize different phases of agent thinking for better
/// UI rendering and user understanding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

impl ReasoningStepType {
    /// Get a human-readable label for this step type
    pub fn label(&self) -> &'static str {
        match self {
            Self::Observation => "Observing",
            Self::Analysis => "Analyzing",
            Self::Planning => "Planning",
            Self::Decision => "Deciding",
            Self::Reflection => "Reflecting",
            Self::RiskAssessment => "Assessing Risks",
            Self::General => "Thinking",
        }
    }

    /// Get an emoji representation for this step type
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Observation => "👁️",
            Self::Analysis => "🔍",
            Self::Planning => "📋",
            Self::Decision => "✅",
            Self::Reflection => "🤔",
            Self::RiskAssessment => "⚠️",
            Self::General => "💭",
        }
    }
}

/// Confidence level for a reasoning step or decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl ConfidenceLevel {
    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::High => "High confidence - strong evidence supports this",
            Self::Medium => "Medium confidence - reasonably certain",
            Self::Low => "Low confidence - some uncertainty exists",
            Self::Exploratory => "Exploratory - experimental approach",
        }
    }

    /// Get a numeric score (0.0 - 1.0)
    pub fn score(&self) -> f32 {
        match self {
            Self::High => 0.9,
            Self::Medium => 0.7,
            Self::Low => 0.4,
            Self::Exploratory => 0.2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reasoning_step_type_default() {
        assert_eq!(ReasoningStepType::default(), ReasoningStepType::General);
    }

    #[test]
    fn test_confidence_level_default() {
        assert_eq!(ConfidenceLevel::default(), ConfidenceLevel::Medium);
    }

    #[test]
    fn test_confidence_scores() {
        assert!(ConfidenceLevel::High.score() > ConfidenceLevel::Medium.score());
        assert!(ConfidenceLevel::Medium.score() > ConfidenceLevel::Low.score());
        assert!(ConfidenceLevel::Low.score() > ConfidenceLevel::Exploratory.score());
    }

    #[test]
    fn test_serde_roundtrip() {
        let step = ReasoningStepType::Analysis;
        let json = serde_json::to_string(&step).unwrap();
        assert_eq!(json, "\"analysis\"");
        let parsed: ReasoningStepType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, step);

        let conf = ConfidenceLevel::High;
        let json = serde_json::to_string(&conf).unwrap();
        assert_eq!(json, "\"high\"");
        let parsed: ConfidenceLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, conf);
    }
}
