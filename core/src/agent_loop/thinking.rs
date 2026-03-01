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
// ThinkingParser
// ============================================================================

/// Parser for extracting structured thinking from LLM reasoning text
///
/// The parser uses heuristics to identify semantic reasoning steps,
/// confidence signals, alternatives, and uncertainties from the AI's
/// raw reasoning output.
pub struct ThinkingParser;

impl ThinkingParser {
    /// Parse raw reasoning text into structured thinking
    pub fn parse(reasoning: &str) -> StructuredThinking {
        StructuredThinking {
            reasoning: reasoning.to_string(),
            steps: Self::extract_steps(reasoning),
            confidence: Self::detect_confidence(reasoning),
            alternatives_considered: Self::extract_alternatives(reasoning),
            uncertainties: Self::extract_uncertainties(reasoning),
            thinking_duration_ms: None,
        }
    }

    /// Extract semantic steps from reasoning text
    fn extract_steps(reasoning: &str) -> Option<Vec<ReasoningStep>> {
        let mut steps = Vec::new();
        let mut current_step: Option<(ReasoningStepType, String, Vec<String>)> = None;

        for line in reasoning.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let step_type = Self::classify_line(line);

            match &mut current_step {
                Some((current_type, content, _lines)) if *current_type == step_type => {
                    // Continue current step
                    content.push(' ');
                    content.push_str(line);
                }
                Some((prev_type, content, _lines)) => {
                    // Save previous step and start new one
                    steps.push(ReasoningStep {
                        label: prev_type.default_label().to_string(),
                        content: content.clone(),
                        step_type: *prev_type,
                        substeps: Vec::new(),
                    });
                    current_step = Some((step_type, line.to_string(), Vec::new()));
                }
                None => {
                    current_step = Some((step_type, line.to_string(), Vec::new()));
                }
            }
        }

        // Don't forget the last step
        if let Some((step_type, content, _)) = current_step {
            steps.push(ReasoningStep {
                label: step_type.default_label().to_string(),
                content,
                step_type,
                substeps: Vec::new(),
            });
        }

        if steps.is_empty() {
            None
        } else {
            Some(steps)
        }
    }

    /// Classify a line into a reasoning step type based on patterns
    pub fn classify_line(line: &str) -> ReasoningStepType {
        let lower = line.to_lowercase();

        // Observation patterns
        if lower.starts_with("looking at")
            || lower.starts_with("i see")
            || lower.starts_with("i notice")
            || lower.starts_with("the current")
            || lower.starts_with("observing")
            || lower.starts_with("the user")
            || lower.starts_with("from the")
            || lower.starts_with("based on")
        {
            return ReasoningStepType::Observation;
        }

        // Analysis patterns
        if lower.starts_with("considering")
            || lower.starts_with("the options")
            || lower.starts_with("comparing")
            || lower.starts_with("analyzing")
            || lower.starts_with("weighing")
            || lower.starts_with("there are")
            || lower.contains("trade-off")
            || lower.contains("tradeoff")
            || lower.contains("pros and cons")
        {
            return ReasoningStepType::Analysis;
        }

        // Planning patterns
        if lower.starts_with("i'll")
            || lower.starts_with("i will")
            || lower.starts_with("my plan")
            || lower.starts_with("the approach")
            || lower.starts_with("first,")
            || lower.starts_with("then,")
            || lower.starts_with("next,")
            || lower.starts_with("to do this")
            || lower.starts_with("my strategy")
        {
            return ReasoningStepType::Planning;
        }

        // Decision patterns
        if lower.starts_with("therefore")
            || lower.starts_with("so i")
            || lower.starts_with("decision:")
            || lower.starts_with("i've decided")
            || lower.starts_with("my decision")
            || lower.starts_with("in conclusion")
            || lower.starts_with("the best")
        {
            return ReasoningStepType::Decision;
        }

        // Reflection patterns
        if lower.starts_with("however")
            || lower.starts_with("but")
            || lower.starts_with("on second thought")
            || lower.starts_with("wait")
            || lower.starts_with("actually")
            || lower.starts_with("let me reconsider")
            || lower.contains("on the other hand")
        {
            return ReasoningStepType::Reflection;
        }

        // Risk assessment patterns
        if lower.contains("risk")
            || lower.contains("careful")
            || lower.contains("warning")
            || lower.contains("caution")
            || lower.contains("danger")
            || lower.starts_with("note:")
            || lower.starts_with("important:")
        {
            return ReasoningStepType::RiskAssessment;
        }

        ReasoningStepType::General
    }

    /// Detect confidence level from reasoning text
    pub fn detect_confidence(reasoning: &str) -> Option<ConfidenceLevel> {
        let lower = reasoning.to_lowercase();

        // High confidence signals
        if lower.contains("i'm confident")
            || lower.contains("i am confident")
            || lower.contains("clearly")
            || lower.contains("definitely")
            || lower.contains("certainly")
            || lower.contains("without doubt")
            || lower.contains("obvious")
        {
            return Some(ConfidenceLevel::High);
        }

        // Low confidence signals
        if lower.contains("not sure")
            || lower.contains("uncertain")
            || lower.contains("not certain")
            || lower.contains("i'm unsure")
        {
            return Some(ConfidenceLevel::Low);
        }

        // Exploratory signals
        if lower.contains("experiment")
            || lower.contains("let's try")
            || lower.contains("explore")
            || lower.contains("see what happens")
            || lower.contains("worth trying")
        {
            return Some(ConfidenceLevel::Exploratory);
        }

        // Medium confidence signals (weaker words)
        if lower.contains("i think")
            || lower.contains("probably")
            || lower.contains("likely")
            || lower.contains("should work")
            || lower.contains("seems like")
        {
            return Some(ConfidenceLevel::Medium);
        }

        None
    }

    /// Extract alternative approaches mentioned
    pub fn extract_alternatives(reasoning: &str) -> Vec<String> {
        let mut alternatives = Vec::new();
        let lower = reasoning.to_lowercase();

        let markers = [
            "alternatively",
            "another option",
            "could also",
            "or we could",
            "other approach",
            "another way",
        ];

        for marker in markers {
            if let Some(pos) = lower.find(marker) {
                // Use original string if byte positions are valid char boundaries,
                // otherwise fall back to lowercased string for safety
                let (source, source_len) = if reasoning.len() == lower.len()
                    && reasoning.is_char_boundary(pos)
                {
                    (reasoning, reasoning.len())
                } else {
                    (lower.as_str(), lower.len())
                };
                let start = source[..pos].rfind('.').map(|p| p + 1).unwrap_or(0);
                let end = source[pos..]
                    .find('.')
                    .map(|p| pos + p + 1)
                    .unwrap_or(source_len);
                let sentence = source[start..end].trim();
                if !sentence.is_empty() && !alternatives.contains(&sentence.to_string()) {
                    alternatives.push(sentence.to_string());
                }
            }
        }

        alternatives
    }

    /// Extract uncertainties or knowledge gaps
    pub fn extract_uncertainties(reasoning: &str) -> Vec<String> {
        let mut uncertainties = Vec::new();
        let lower = reasoning.to_lowercase();

        let markers = [
            "i'm not sure",
            "i am not sure",
            "i'm uncertain",
            "i am uncertain",
            "uncertain about",
            "unclear",
            "don't know",
            "need to verify",
            "assumption",
            "might be wrong",
            "not certain",
            "can't tell",
        ];

        for marker in markers {
            if let Some(pos) = lower.find(marker) {
                // Use original string if byte positions are valid char boundaries,
                // otherwise fall back to lowercased string for safety
                let (source, source_len) = if reasoning.len() == lower.len()
                    && reasoning.is_char_boundary(pos)
                {
                    (reasoning, reasoning.len())
                } else {
                    (lower.as_str(), lower.len())
                };
                let start = source[..pos].rfind('.').map(|p| p + 1).unwrap_or(0);
                let end = source[pos..]
                    .find('.')
                    .map(|p| pos + p + 1)
                    .unwrap_or(source_len);
                let sentence = source[start..end].trim();
                if !sentence.is_empty() && !uncertainties.contains(&sentence.to_string()) {
                    uncertainties.push(sentence.to_string());
                }
            }
        }

        uncertainties
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

    // ========================================================================
    // ThinkingParser tests
    // ========================================================================

    #[test]
    fn test_parser_basic() {
        let reasoning = "I see the user wants to add a feature. I'll implement it step by step.";
        let thinking = ThinkingParser::parse(reasoning);

        assert_eq!(thinking.reasoning, reasoning);
        assert!(thinking.has_steps());
    }

    #[test]
    fn test_parser_classify_observation() {
        assert_eq!(
            ThinkingParser::classify_line("Looking at the code, I notice a pattern"),
            ReasoningStepType::Observation
        );
        assert_eq!(
            ThinkingParser::classify_line("I see that the function is complex"),
            ReasoningStepType::Observation
        );
        assert_eq!(
            ThinkingParser::classify_line("The user wants to implement caching"),
            ReasoningStepType::Observation
        );
    }

    #[test]
    fn test_parser_classify_analysis() {
        assert_eq!(
            ThinkingParser::classify_line("Considering the options available"),
            ReasoningStepType::Analysis
        );
        assert_eq!(
            ThinkingParser::classify_line("There are trade-offs to consider"),
            ReasoningStepType::Analysis
        );
    }

    #[test]
    fn test_parser_classify_planning() {
        assert_eq!(
            ThinkingParser::classify_line("I'll start by creating the struct"),
            ReasoningStepType::Planning
        );
        assert_eq!(
            ThinkingParser::classify_line("First, we need to define the interface"),
            ReasoningStepType::Planning
        );
    }

    #[test]
    fn test_parser_classify_decision() {
        assert_eq!(
            ThinkingParser::classify_line("Therefore, I will use Redis"),
            ReasoningStepType::Decision
        );
        assert_eq!(
            ThinkingParser::classify_line("The best approach is to use a trait"),
            ReasoningStepType::Decision
        );
    }

    #[test]
    fn test_parser_classify_reflection() {
        assert_eq!(
            ThinkingParser::classify_line("However, this might not work"),
            ReasoningStepType::Reflection
        );
        assert_eq!(
            ThinkingParser::classify_line("Wait, let me reconsider"),
            ReasoningStepType::Reflection
        );
    }

    #[test]
    fn test_parser_detect_confidence_high() {
        let confidence = ThinkingParser::detect_confidence(
            "I'm confident this approach will work. It's clearly the best option.",
        );
        assert_eq!(confidence, Some(ConfidenceLevel::High));
    }

    #[test]
    fn test_parser_detect_confidence_low() {
        let confidence =
            ThinkingParser::detect_confidence("I'm not sure if this is the right approach.");
        assert_eq!(confidence, Some(ConfidenceLevel::Low));
    }

    #[test]
    fn test_parser_detect_confidence_exploratory() {
        let confidence =
            ThinkingParser::detect_confidence("Let's try this approach and see what happens.");
        assert_eq!(confidence, Some(ConfidenceLevel::Exploratory));
    }

    #[test]
    fn test_parser_extract_alternatives() {
        let reasoning = "I could use Redis. Alternatively, we could use an in-memory cache. Another option would be file-based caching.";
        let alternatives = ThinkingParser::extract_alternatives(reasoning);

        assert!(!alternatives.is_empty());
        assert!(alternatives.iter().any(|a| a.contains("Alternatively")));
    }

    #[test]
    fn test_parser_extract_uncertainties() {
        let reasoning = "The approach looks good. I'm not sure about the edge case handling. Need to verify the API supports this.";
        let uncertainties = ThinkingParser::extract_uncertainties(reasoning);

        assert!(!uncertainties.is_empty());
    }

    #[test]
    fn test_parser_full_reasoning() {
        let reasoning = r#"
Looking at the request, the user wants to add caching.

Considering the options: Redis vs in-memory vs file-based.
There are trade-offs between complexity and performance.

I'll implement an in-memory LRU cache first.
First, I'll define the Cache trait.
Then, implement the LruCache struct.

I think this should work well for most cases.
It will probably perform adequately under normal load.

Therefore, I'll proceed with this approach.
"#;

        let thinking = ThinkingParser::parse(reasoning);

        assert!(thinking.has_steps());
        assert!(thinking.step_count() >= 3);
        assert_eq!(thinking.confidence, Some(ConfidenceLevel::Medium));
    }
}
