//! Tiered validation components for skill evolution.
//!
//! Provides risk profiling, test set generation, structural linting (L1),
//! and semantic replay (L2) for evolved patterns.

pub mod risk_profiler;
pub mod test_set_generator;
pub mod structural_linter;
pub mod semantic_replayer;
pub mod tiered_validator;
pub mod shadow_fs;
pub mod restricted_tools;

pub use risk_profiler::{SkillRiskLevel, SkillRiskProfile, SkillRiskProfiler};
pub use test_set_generator::{SampleSource, TestSample, TestSetGenerator, ValidationTestSet};
pub use structural_linter::{LintResult, StructuralLinter};
pub use semantic_replayer::{ReplayResult, SemanticReplayer};
pub use tiered_validator::{TieredValidator, ValidationLevel, ValidationVerdict};
