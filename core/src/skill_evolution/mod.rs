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

pub mod types;

pub use types::{
    CommitResult, ExecutionStatus, GenerationResult, SkillExecution, SkillMetrics,
    SolidificationConfig, SolidificationSuggestion,
};
