//! Skill evolution system.
//!
//! Tracks skill executions, detects patterns, and suggests solidification
//! of repeated successful patterns into permanent skills.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
//! │ EvolutionTracker│────▶│SolidificationDet│────▶│  SkillGenerator │
//! │  (Log Executions)│     │ (Check Thresholds)│    │ (Create SKILL.md)│
//! └─────────────────┘     └─────────────────┘     └────────┬────────┘
//!                                                          │
//!                                                          ▼
//!                                                  ┌─────────────────┐
//!                                                  │   GitCommitter  │
//!                                                  │ (Auto-commit)   │
//!                                                  └─────────────────┘
//! ```
//!
//! ## Pipeline Usage
//!
//! For end-to-end solidification, use the `SolidificationPipeline`:
//!
//! ```rust,ignore
//! use alephcore::skill_evolution::{SolidificationPipeline, EvolutionTracker};
//!
//! let tracker = Arc::new(EvolutionTracker::new("evolution.db")?);
//! let pipeline = SolidificationPipeline::new(tracker);
//!
//! // Run detection and get suggestions
//! let result = pipeline.run().await?;
//! for suggestion in result.suggestions {
//!     println!("Suggest: {}", suggestion.suggested_name);
//! }
//! ```

pub mod approval;
pub mod compiler;
pub mod detector;
pub mod generator;
pub mod git;
pub mod pipeline;
pub mod safety;
pub mod tool_generator;
pub mod tool_testing;
pub mod tracker;
pub mod types;

pub use approval::{ApprovalConfig, ApprovalManager, ApprovalRequest, ApprovalStatus};
pub use compiler::{CompilationResult, CompilerStatus, SkillCompiler};
pub use detector::SolidificationDetector;
pub use generator::SkillGenerator;
pub use git::GitCommitter;
pub use pipeline::{PipelineResult, PipelineStatus, SolidificationPipeline};
pub use safety::{
    ConcernType, FirstRunConfirmation, SafetyConcern, SafetyGate, SafetyGateConfig, SafetyLevel,
    SafetyReport,
};
pub use tool_generator::{
    GeneratedToolDefinition, GenerationMetadata, ToolGenerationResult, ToolGenerator,
    ToolGeneratorConfig,
};
pub use tool_testing::{SelfTestReport, SubprocessTool, TestResult, ToolRegistrar, ToolTester};
pub use tracker::EvolutionTracker;
pub use types::{
    CommitResult, ExecutionStatus, GenerationResult, SkillExecution, SkillMetrics,
    SolidificationConfig, SolidificationSuggestion,
};
