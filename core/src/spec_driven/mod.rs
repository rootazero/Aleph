//! Spec-driven development workflow.
//!
//! This module implements an automated development workflow:
//! 1. SpecWriter: Generate specifications from requirements
//! 2. TestWriter: Generate test cases from specifications
//! 3. LlmJudge: Evaluate implementations against specs
//! 4. Workflow: Orchestrate the entire cycle with retry logic
//! 5. YamlSpec: YAML-based behavior specifications for BDD
//! 6. Runner: Unified test runner for dual-track testing

pub mod judge;
pub mod runner;
pub mod spec_writer;
pub mod test_writer;
pub mod types;
pub mod workflow;
pub mod yaml_spec;

pub use judge::LlmJudge;
pub use spec_writer::SpecWriter;
pub use test_writer::TestWriter;
pub use workflow::{NoOpWorkflowCallback, SpecDrivenWorkflow, WorkflowCallback};
pub use types::{
    AssertionType, EvaluationResult, Spec, SpecMetadata, SpecTarget, TestCase, TestResult,
    TestType, WorkflowConfig, WorkflowResult,
};
pub use yaml_spec::{YamlSpec, YamlSpecError, Scenario, LlmJudgeConfig};
pub use runner::{UnifiedTestRunner, RunnerConfig, UnifiedResult, TestSource, RunnerError};
