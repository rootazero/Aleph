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
//! ## Type Categories
//!
//! ### Success Criteria
//! - [`SuccessManifest`]: Defines what success looks like for a task
//! - [`ValidationRule`]: Individual validation conditions (file exists, command passes, etc.)
//! - [`SoftMetric`]: Weighted rules that contribute to quality score
//!
//! ### Evaluation Results
//! - [`Verdict`]: Overall result of evaluating a manifest
//! - [`RuleResult`]: Result of a single hard constraint
//! - [`SoftRuleResult`]: Result of a single soft metric
//! - [`JudgeTarget`]: Target for semantic (LLM-based) evaluation
//! - [`ModelTier`]: Model tier for LLM-based evaluation
//!
//! ### Execution
//! - [`PoeTask`]: A task with manifest and instruction
//! - [`PoeOutcome`]: Final outcome (Success, StrategySwitch, BudgetExhausted)
//! - [`WorkerOutput`]: Output from worker execution
//! - [`WorkerState`]: Final state of a worker (Completed, Failed, NeedsInput)
//! - [`Artifact`]: A file produced during execution
//! - [`ChangeType`]: Type of file change (Created, Modified, Deleted)
//! - [`StepLog`]: Log entry for a single execution step
//!
//! ### Budget Management
//! - [`PoeBudget`]: Tracks token usage and attempts
//! - [`BudgetStatus`]: Current budget status (Improving, Stable, Degrading, Stuck, Exhausted)
//!
//! ### Validation
//! - [`HardValidator`]: Deterministic validation (file checks, commands)
//! - [`SemanticValidator`]: LLM-based quality evaluation
//! - [`CompositeValidator`]: Two-phase validation pipeline
//!
//! ### Worker Abstraction
//! - [`Worker`]: Trait for executing instructions
//! - [`AgentLoopWorker`]: Worker that integrates with AgentLoop
//! - [`StateSnapshot`]: Workspace state for rollback
//!
//! ### Experience (Future)
//! - [`Experience`]: Crystallized experience from past execution
//! - [`TaskPattern`]: Pattern that matches similar tasks
//! - [`SolutionPath`]: Solution path that worked
//! - [`ExperienceOutcome`]: Outcome metrics for an experience
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::poe::{PoeManager, PoeConfig, PoeTask, SuccessManifest, ValidationRule};
//! use aethecore::poe::{AgentLoopWorker, CompositeValidator};
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
//! let manifest = SuccessManifest::new("task-1", "Add authentication")
//!     .with_hard_constraint(ValidationRule::CommandPasses {
//!         cmd: "cargo".into(),
//!         args: vec!["test".into()],
//!         timeout_ms: 60_000,
//!     })
//!     .with_max_attempts(5);
//!
//! let task = PoeTask::new(manifest, "Implement JWT auth");
//! let outcome = manager.execute(task).await?;
//!
//! match outcome {
//!     aethecore::poe::PoeOutcome::Success(verdict) => {
//!         println!("Task succeeded! Distance: {}", verdict.distance_score);
//!     }
//!     aethecore::poe::PoeOutcome::StrategySwitch { reason, suggestion } => {
//!         println!("Stuck: {}. Try: {}", reason, suggestion);
//!     }
//!     aethecore::poe::PoeOutcome::BudgetExhausted { attempts, last_error } => {
//!         println!("Failed after {} attempts: {}", attempts, last_error);
//!     }
//! }
//! ```

pub mod budget;
pub mod manager;
pub mod types;
pub mod validation;
pub mod worker;

// Re-exports for convenient access
// Budget management
pub use budget::{BudgetStatus, PoeBudget};

// Manager and configuration
pub use manager::{PoeConfig, PoeManager};

// Core types
pub use types::{
    // Success criteria
    SuccessManifest, SoftMetric, ValidationRule,
    // Evaluation targets and tiers
    JudgeTarget, ModelTier,
    // Evaluation results
    Verdict, RuleResult, SoftRuleResult,
    // Execution outputs
    WorkerOutput, WorkerState, Artifact, ChangeType, StepLog,
    // Task and outcome
    PoeTask, PoeOutcome,
    // Experience types (for future crystallization)
    Experience, TaskPattern, SolutionPath, ExperienceOutcome,
};

// Validation
pub use validation::{CompositeValidator, HardValidator, SemanticValidator};

// Worker abstraction
pub use worker::{AgentLoopWorker, StateSnapshot, Worker};

// Integration tests
#[cfg(test)]
mod tests;
