//! POE (Principle-Operation-Evaluation) Architecture
//!
//! A goal-oriented agent execution framework that:
//! 1. **Principle**: Defines success criteria before execution (SuccessManifest)
//! 2. **Operation**: Executes with heuristic guidance (Worker abstraction)
//! 3. **Evaluation**: Validates results with mixed hard/semantic checks

pub mod budget;
pub mod manager;
pub mod types;
pub mod validation;
pub mod worker;

// Re-exports for convenient access
pub use budget::{BudgetStatus, PoeBudget};
pub use types::{
    Artifact, ChangeType, Experience, ExperienceOutcome, JudgeTarget, ModelTier, PoeOutcome,
    PoeTask, RuleResult, SoftMetric, SoftRuleResult, SolutionPath, StepLog, SuccessManifest,
    TaskPattern, ValidationRule, Verdict, WorkerOutput, WorkerState,
};
pub use validation::HardValidator;
pub use worker::{AgentLoopWorker, StateSnapshot, Worker};
