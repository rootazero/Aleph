//! Core types for spec-driven development workflow.
//!
//! Defines the data structures for specs, tests, and evaluations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A specification describing what needs to be implemented.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// Unique identifier
    pub id: String,
    /// Short title
    pub title: String,
    /// Detailed description
    pub description: String,
    /// List of acceptance criteria (must be verifiable)
    pub acceptance_criteria: Vec<String>,
    /// Implementation hints and constraints
    pub implementation_notes: Option<String>,
    /// Target language/framework
    pub target: SpecTarget,
    /// Metadata
    pub metadata: SpecMetadata,
}

/// Target for the specification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecTarget {
    /// Programming language (rust, python, typescript, etc.)
    pub language: String,
    /// Framework if applicable
    pub framework: Option<String>,
    /// Output file path
    pub output_path: Option<String>,
}

/// Spec metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecMetadata {
    /// Creation timestamp
    pub created_at: Option<u64>,
    /// Original requirement text
    pub original_requirement: String,
    /// Number of iterations
    pub iteration: u32,
}

impl Spec {
    /// Create a new spec with basic fields
    pub fn new(id: impl Into<String>, title: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            acceptance_criteria: Vec::new(),
            implementation_notes: None,
            target: SpecTarget::default(),
            metadata: SpecMetadata::default(),
        }
    }

    /// Add an acceptance criterion
    pub fn with_criterion(mut self, criterion: impl Into<String>) -> Self {
        self.acceptance_criteria.push(criterion.into());
        self
    }

    /// Set target language
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.target.language = language.into();
        self
    }
}

/// A test case for validating implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    /// Test name
    pub name: String,
    /// Test description
    pub description: String,
    /// Test type (unit, integration, e2e)
    pub test_type: TestType,
    /// Input data
    pub input: serde_json::Value,
    /// Expected output
    pub expected: serde_json::Value,
    /// Assertion type
    pub assertion: AssertionType,
    /// Whether this is an edge case
    pub is_edge_case: bool,
}

/// Type of test
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TestType {
    #[default]
    Unit,
    Integration,
    E2e,
}

/// Type of assertion
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionType {
    #[default]
    Equals,
    Contains,
    Matches,
    GreaterThan,
    LessThan,
    NotNull,
    Throws,
}

impl TestCase {
    /// Create a new test case
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            test_type: TestType::default(),
            input: serde_json::Value::Null,
            expected: serde_json::Value::Null,
            assertion: AssertionType::default(),
            is_edge_case: false,
        }
    }

    /// Set as unit test
    pub fn unit(mut self) -> Self {
        self.test_type = TestType::Unit;
        self
    }

    /// Set as edge case
    pub fn edge_case(mut self) -> Self {
        self.is_edge_case = true;
        self
    }
}

/// Result of running tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// Test case name
    pub test_name: String,
    /// Whether test passed
    pub passed: bool,
    /// Actual output if available
    pub actual_output: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution time in milliseconds
    pub duration_ms: u64,
}

/// Evaluation result from LlmJudge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Overall score (0.0 to 1.0)
    pub score: f32,
    /// Per-criterion scores
    pub criterion_scores: HashMap<String, f32>,
    /// Detailed feedback
    pub feedback: String,
    /// Suggestions for improvement
    pub suggestions: Vec<String>,
    /// Whether implementation is acceptable
    pub is_acceptable: bool,
}

impl EvaluationResult {
    /// Create a passing evaluation
    pub fn passing(score: f32, feedback: impl Into<String>) -> Self {
        Self {
            score,
            criterion_scores: HashMap::new(),
            feedback: feedback.into(),
            suggestions: Vec::new(),
            is_acceptable: score >= 0.8,
        }
    }

    /// Create a failing evaluation
    pub fn failing(score: f32, feedback: impl Into<String>, suggestions: Vec<String>) -> Self {
        Self {
            score,
            criterion_scores: HashMap::new(),
            feedback: feedback.into(),
            suggestions,
            is_acceptable: false,
        }
    }
}

/// Result of the entire workflow
#[derive(Debug, Clone)]
pub enum WorkflowResult {
    /// Implementation succeeded
    Success {
        spec: Spec,
        tests: Vec<TestCase>,
        evaluation: EvaluationResult,
    },
    /// Needs another iteration
    NeedsIteration {
        iteration: u32,
        feedback: String,
        suggestions: Vec<String>,
    },
    /// Failed after max iterations
    Failed {
        reason: String,
        last_evaluation: Option<EvaluationResult>,
    },
}

/// Workflow configuration
#[derive(Debug, Clone)]
pub struct WorkflowConfig {
    /// Maximum number of iterations
    pub max_iterations: u32,
    /// Minimum acceptable score (0.0 to 1.0)
    pub min_score: f32,
    /// Timeout per phase in seconds
    pub phase_timeout_secs: u64,
    /// Whether to auto-commit on success
    pub auto_commit: bool,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            max_iterations: 3,
            min_score: 0.8,
            phase_timeout_secs: 300,
            auto_commit: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_builder() {
        let spec = Spec::new("spec-001", "Add User", "Implement user registration")
            .with_criterion("Email must be validated")
            .with_criterion("Password must be hashed")
            .with_language("rust");

        assert_eq!(spec.id, "spec-001");
        assert_eq!(spec.acceptance_criteria.len(), 2);
        assert_eq!(spec.target.language, "rust");
    }

    #[test]
    fn test_test_case_builder() {
        let test = TestCase::new("test_empty_input", "Should handle empty string")
            .unit()
            .edge_case();

        assert_eq!(test.test_type, TestType::Unit);
        assert!(test.is_edge_case);
    }

    #[test]
    fn test_evaluation_result() {
        let passing = EvaluationResult::passing(0.95, "Excellent implementation");
        assert!(passing.is_acceptable);
        assert_eq!(passing.score, 0.95);

        let failing = EvaluationResult::failing(0.5, "Needs work", vec!["Fix validation".into()]);
        assert!(!failing.is_acceptable);
    }

    #[test]
    fn test_workflow_config_default() {
        let config = WorkflowConfig::default();
        assert_eq!(config.max_iterations, 3);
        assert_eq!(config.min_score, 0.8);
    }
}
