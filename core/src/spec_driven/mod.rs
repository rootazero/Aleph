//! Spec-driven development workflow.
//!
//! This module implements an automated development workflow:
//! 1. SpecWriter: Generate specifications from requirements
//! 2. TestWriter: Generate test cases from specifications
//! 3. LlmJudge: Evaluate implementations against specs
//! 4. Workflow: Orchestrate the entire cycle with retry logic

pub mod judge;
pub mod spec_writer;
pub mod test_writer;
pub mod types;

pub use judge::LlmJudge;
pub use spec_writer::SpecWriter;
pub use test_writer::TestWriter;
pub use types::{
    AssertionType, EvaluationResult, Spec, SpecMetadata, SpecTarget, TestCase, TestResult,
    TestType, WorkflowConfig, WorkflowResult,
};
