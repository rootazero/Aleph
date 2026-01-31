//! POE (Principle-Operation-Evaluation) Architecture
//!
//! A goal-oriented agent execution framework that:
//! 1. **Principle**: Defines success criteria before execution (SuccessManifest)
//! 2. **Operation**: Executes with heuristic guidance (Worker abstraction)
//! 3. **Evaluation**: Validates results with mixed hard/semantic checks
//!
//! ## Core Components
//!
//! - **PoeManager**: The main orchestrator that runs the P->O->E cycle
//! - **PoeConfig**: Configuration for budget limits and stuck detection
//! - **SuccessManifest**: Defines success criteria for a task
//! - **Worker**: Trait for executing instructions (AgentLoopWorker, MockWorker)
//! - **CompositeValidator**: Two-phase validation (hard + semantic)
//! - **PoeBudget**: Tracks token usage and attempts with entropy-based stuck detection
//!
//! ## Example
//!
//! ```rust,ignore
//! use aethecore::poe::{PoeManager, PoeConfig, PoeTask, SuccessManifest, ValidationRule};
//! use aethecore::poe::worker::AgentLoopWorker;
//! use aethecore::poe::validation::CompositeValidator;
//!
//! // Create components
//! let worker = AgentLoopWorker::new("/workspace".into());
//! let validator = CompositeValidator::new(provider);
//! let config = PoeConfig::default();
//!
//! // Create manager
//! let manager = PoeManager::new(worker, validator, config);
//!
//! // Define task with success criteria
//! let manifest = SuccessManifest::new("create-project", "Create a Rust project")
//!     .with_hard_constraint(ValidationRule::FileExists { path: "Cargo.toml".into() })
//!     .with_max_attempts(5);
//! let task = PoeTask::new(manifest, "Create a new Rust project with cargo init");
//!
//! // Execute
//! let outcome = manager.execute(task).await?;
//! ```

pub mod budget;
pub mod manager;
pub mod types;
pub mod validation;
pub mod worker;

// Re-exports for convenient access
pub use budget::{BudgetStatus, PoeBudget};
pub use manager::{PoeConfig, PoeManager};
pub use types::{
    Artifact, ChangeType, Experience, ExperienceOutcome, JudgeTarget, ModelTier, PoeOutcome,
    PoeTask, RuleResult, SoftMetric, SoftRuleResult, SolutionPath, StepLog, SuccessManifest,
    TaskPattern, ValidationRule, Verdict, WorkerOutput, WorkerState,
};
pub use validation::HardValidator;
pub use worker::{AgentLoopWorker, StateSnapshot, Worker};
