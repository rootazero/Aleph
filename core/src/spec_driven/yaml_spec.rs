//! YAML-based behavior specification format.
//!
//! This module provides types and parsing for YAML spec files,
//! enabling BDD-style behavior specifications with both deterministic
//! assertions and LLM-based semantic validation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::types::{AssertionType, TestCase, TestType};

/// A YAML-based behavior specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlSpec {
    /// Spec name
    pub name: String,
    /// Version string
    pub version: String,
    /// Context information
    pub context: SpecContext,
    /// List of scenarios
    pub scenarios: Vec<Scenario>,
    /// Optional metadata
    #[serde(default)]
    pub metadata: SpecMetadata,
}

/// Context for the specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecContext {
    /// Description of what this spec validates
    pub description: String,
    /// Target aggregate root (if DDD)
    #[serde(default)]
    pub aggregate_root: Option<String>,
    /// Target module
    #[serde(default)]
    pub module: Option<String>,
}

/// Metadata for the specification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecMetadata {
    /// Creation date
    #[serde(default)]
    pub created: Option<String>,
    /// Author
    #[serde(default)]
    pub author: Option<String>,
    /// Related feature file
    #[serde(default)]
    pub related_feature: Option<String>,
}

/// A single scenario in the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Scenario name
    pub name: String,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
    /// Given conditions (preconditions)
    pub given: Vec<GivenCondition>,
    /// When action (trigger)
    pub when: WhenAction,
    /// Then assertions (expected outcomes)
    pub then: Vec<ThenAssertion>,
}

/// A precondition in Given clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GivenCondition {
    /// Task graph setup
    TaskGraph {
        task_graph: TaskGraphSetup,
    },
    /// Mock executor setup
    MockExecutor {
        mock_executor: MockExecutorSetup,
    },
    /// User approval state
    UserApproval {
        user_approval: String,
    },
    /// Generic key-value condition
    Generic(HashMap<String, serde_json::Value>),
}

/// Task graph setup for testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraphSetup {
    /// Graph ID
    pub id: String,
    /// Tasks in the graph
    pub tasks: Vec<TaskSetup>,
    /// Dependencies between tasks
    #[serde(default)]
    pub dependencies: HashMap<String, Vec<String>>,
}

/// Individual task setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSetup {
    /// Task ID
    pub id: String,
    /// Task name
    pub name: String,
    /// Task status
    #[serde(default = "default_status")]
    pub status: String,
    /// Task type
    #[serde(default)]
    pub task_type: Option<String>,
    /// Risk level
    #[serde(default)]
    pub risk_level: Option<String>,
    /// Progress (0.0 to 1.0)
    #[serde(default)]
    pub progress: Option<f32>,
}

fn default_status() -> String {
    "Pending".to_string()
}

/// Mock executor setup for testing retry behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockExecutorSetup {
    /// Task ID to mock
    pub task_id: String,
    /// Number of times to fail before succeeding
    #[serde(default)]
    pub fail_count: u32,
    /// Whether to succeed after failures
    #[serde(default)]
    pub then_succeed: bool,
}

/// Action trigger in When clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhenAction {
    /// Action name
    pub action: String,
    /// Optional configuration
    #[serde(default)]
    pub config: Option<ActionConfig>,
}

/// Configuration for actions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionConfig {
    /// Maximum parallelism
    #[serde(default)]
    pub max_parallelism: Option<u32>,
    /// Maximum retries
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Initial backoff in milliseconds
    #[serde(default)]
    pub initial_backoff_ms: Option<u64>,
}

/// An assertion in Then clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ThenAssertion {
    /// Deterministic assertion
    Deterministic {
        assertion_type: String,
        #[serde(flatten)]
        params: HashMap<String, serde_json::Value>,
    },
    /// LLM-based semantic validation
    LlmJudge {
        llm_judge: LlmJudgeConfig,
    },
}

/// Configuration for LLM-based validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmJudgeConfig {
    /// Validation prompt
    pub prompt: String,
    /// Success criteria
    pub criteria: String,
    /// Required evidence
    #[serde(default)]
    pub evidence_required: Vec<String>,
}

// ============================================================================
// Parsing and Conversion
// ============================================================================

impl YamlSpec {
    /// Load a YAML spec from a file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, YamlSpecError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| YamlSpecError::IoError(e.to_string()))?;
        Self::parse(&content)
    }

    /// Parse a YAML spec from a string.
    pub fn parse(content: &str) -> Result<Self, YamlSpecError> {
        serde_yaml::from_str(content)
            .map_err(|e| YamlSpecError::ParseError(e.to_string()))
    }

    /// Convert scenarios to TestCase format for spec_driven workflow.
    pub fn to_test_cases(&self) -> Vec<TestCase> {
        self.scenarios
            .iter()
            .map(|scenario| scenario.to_test_case(&self.context))
            .collect()
    }

    /// Get scenarios that require LLM judge validation.
    pub fn semantic_scenarios(&self) -> Vec<&Scenario> {
        self.scenarios
            .iter()
            .filter(|s| s.has_llm_judge())
            .collect()
    }

    /// Get scenarios with only deterministic assertions.
    pub fn deterministic_scenarios(&self) -> Vec<&Scenario> {
        self.scenarios
            .iter()
            .filter(|s| !s.has_llm_judge())
            .collect()
    }
}

impl Scenario {
    /// Check if this scenario requires LLM judge.
    pub fn has_llm_judge(&self) -> bool {
        self.then.iter().any(|a| matches!(a, ThenAssertion::LlmJudge { .. }))
    }

    /// Convert to TestCase format.
    pub fn to_test_case(&self, context: &SpecContext) -> TestCase {
        let test_type = if self.has_llm_judge() {
            TestType::E2e
        } else {
            TestType::Integration
        };

        let input = self.build_input();
        let expected = self.build_expected();

        TestCase {
            name: self.name.clone(),
            description: self.description.clone().unwrap_or_else(|| {
                format!("Scenario from {}", context.description)
            }),
            test_type,
            input,
            expected,
            assertion: AssertionType::Equals,
            is_edge_case: false,
        }
    }

    fn build_input(&self) -> serde_json::Value {
        let mut input = serde_json::Map::new();

        // Add given conditions
        let given: Vec<serde_json::Value> = self.given
            .iter()
            .map(|g| serde_json::to_value(g).unwrap_or_default())
            .collect();
        input.insert("given".to_string(), serde_json::Value::Array(given));

        // Add when action
        input.insert("when".to_string(),
            serde_json::to_value(&self.when).unwrap_or_default());

        serde_json::Value::Object(input)
    }

    fn build_expected(&self) -> serde_json::Value {
        let then: Vec<serde_json::Value> = self.then
            .iter()
            .map(|t| serde_json::to_value(t).unwrap_or_default())
            .collect();
        serde_json::Value::Array(then)
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur when working with YAML specs.
#[derive(Debug, Clone)]
pub enum YamlSpecError {
    /// IO error reading file
    IoError(String),
    /// YAML parsing error
    ParseError(String),
    /// Validation error
    ValidationError(String),
}

impl std::fmt::Display for YamlSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "IO error: {}", e),
            Self::ParseError(e) => write!(f, "Parse error: {}", e),
            Self::ValidationError(e) => write!(f, "Validation error: {}", e),
        }
    }
}

impl std::error::Error for YamlSpecError {}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SPEC: &str = r#"
name: "Test Spec"
version: "1.0"
context:
  description: "Test description"
  aggregate_root: "TaskGraph"
  module: "dispatcher"

scenarios:
  - name: "Simple test"
    given:
      - task_graph:
          id: "graph-001"
          tasks:
            - { id: "A", name: "Task A" }
          dependencies: {}
    when:
      action: "execute_graph"
    then:
      - assertion_type: "deterministic"
        check: "all_completed"
        expected: true
"#;

    #[test]
    fn test_parse_yaml_spec() {
        let spec = YamlSpec::parse(SAMPLE_SPEC).unwrap();
        assert_eq!(spec.name, "Test Spec");
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.scenarios.len(), 1);
    }

    #[test]
    fn test_scenario_to_test_case() {
        let spec = YamlSpec::parse(SAMPLE_SPEC).unwrap();
        let test_cases = spec.to_test_cases();
        assert_eq!(test_cases.len(), 1);
        assert_eq!(test_cases[0].name, "Simple test");
    }

    #[test]
    fn test_deterministic_vs_semantic() {
        let spec_with_judge = r#"
name: "Mixed Spec"
version: "1.0"
context:
  description: "Test"

scenarios:
  - name: "Deterministic"
    given: []
    when:
      action: "test"
    then:
      - assertion_type: "deterministic"
        check: "pass"
        expected: true

  - name: "Semantic"
    given: []
    when:
      action: "test"
    then:
      - llm_judge:
          prompt: "Validate this"
          criteria: "Must pass"
"#;
        let spec = YamlSpec::parse(spec_with_judge).unwrap();
        assert_eq!(spec.deterministic_scenarios().len(), 1);
        assert_eq!(spec.semantic_scenarios().len(), 1);
    }
}
