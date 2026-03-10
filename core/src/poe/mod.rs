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
//! ### Experience Crystallization
//! - [`ExperienceRecorder`]: Trait for recording execution experiences
//! - [`ChannelCrystallizer`]: Send+Sync crystallizer using channels for async safety
//! - [`CrystallizerWorker`]: Background worker that writes to EvolutionTracker
//! - [`NoOpRecorder`]: No-op implementation for when crystallization is disabled
//! - [`ExperienceCrystallizer`]: Direct crystallizer (not Send, use ChannelCrystallizer in async)
//!
//! ### Experience Types
//! - [`Experience`]: Crystallized experience from past execution
//! - [`TaskPattern`]: Pattern that matches similar tasks
//! - [`SolutionPath`]: Solution path that worked
//! - [`ExperienceOutcome`]: Outcome metrics for an experience
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::poe::{PoeManager, PoeConfig, PoeTask, SuccessManifest, ValidationRule};
//! use alephcore::poe::{AgentLoopWorker, CompositeValidator};
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
//!     alephcore::poe::PoeOutcome::Success(verdict) => {
//!         println!("Task succeeded! Distance: {}", verdict.distance_score);
//!     }
//!     alephcore::poe::PoeOutcome::StrategySwitch { reason, suggestion } => {
//!         println!("Stuck: {}. Try: {}", reason, suggestion);
//!     }
//!     alephcore::poe::PoeOutcome::BudgetExhausted { attempts, last_error } => {
//!         println!("Failed after {} attempts: {}", attempts, last_error);
//!     }
//! }
//! ```

pub mod budget;
pub mod contract;
pub mod contract_store;
pub mod crystallization;
pub mod handler_types;
pub mod interceptor;
pub mod lazy_evaluator;
pub mod manager;
pub mod manifest;
pub mod meta_cognition;
pub mod prompt_context;
pub mod projectors;
pub mod prompt_layer;
pub mod services;
pub mod trust;
pub mod types;
pub mod validation;
pub mod worker;
pub mod event_bus;
pub mod events;

#[cfg(test)]
mod proptest_budget;
#[cfg(test)]
mod proptest_types;

// Re-exports for convenient access
// Lazy evaluator (lightweight POE without full manifest)
pub use lazy_evaluator::{LazyPoeEvaluator, LightManifest};

// Interceptor directives
pub use interceptor::{PoeLoopCallback, StepDirective, StepEvaluator};

// Budget management
pub use budget::{BudgetStatus, PoeBudget};

// Manifest generation
pub use manifest::ManifestBuilder;

// Manager and configuration
pub use manager::{MetaCognitionCallback, PoeConfig, PoeManager, ValidationCallback, ValidationEvent};

// Core types
pub use types::{
    // Success criteria
    SuccessManifest, SoftMetric, ValidationRule,
    // Risk assessment
    BlastRadius, RiskLevel,
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
pub use worker::{
    AgentLoopWorker, GatewayAgentLoopWorker, PlaceholderWorker, StateSnapshot, Worker,
    create_gateway_worker,
};

// Experience crystallization
pub use crystallization::{
    ChannelCrystallizer, CrystallizerWorker, ExperienceCrystallizer,
    ExperienceRecorder, NoOpRecorder,
};

// Crystallization submodules (migrated from Cortex)
pub use crystallization::experience::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus,
    Experience as CortexExperience, ExperienceBuilder, ParameterConfig, ParameterMapping,
};
pub use crystallization::distillation::{
    DistillationConfig, DistillationPriority, DistillationService,
};
pub use crystallization::pattern_extractor::{
    ExtractedPattern, PatternExtractor, PatternExtractorConfig,
};
pub use crystallization::clustering::{
    Cluster, ClusteringConfig, ClusteringService,
};
pub use crystallization::dreaming::{
    CortexDreamingConfig, CortexDreamingService, DreamingMetrics,
};

// Experience store
pub use crystallization::experience_store::{
    ExperienceStore, InMemoryExperienceStore, PoeExperience,
};

// Contract signing workflow
pub use contract::{
    ContractContext, ContractSummary, PendingContract, PendingResult,
    PrepareResult, RejectRequest, RejectResult, SignRequest, SignResult,
};
pub use contract_store::PendingContractStore;

// Prompt context
pub use prompt_context::PoePromptContext;

// Prompt layer (for PromptPipeline injection)
pub use prompt_layer::PoePromptLayer;

// Trust evaluation (progressive auto-approval)
pub use trust::{
    AlwaysRequireSignature, AutoApprovalDecision, ExperienceTrustEvaluator,
    TrustContext, TrustEvaluator, WhitelistTrustEvaluator,
};

// Gateway handler types
pub use handler_types::{
    // Task state
    PoeTaskState, PoeTaskStatus,
    // Events
    PoeAcceptedEvent, PoeStepEvent, PoeValidationEvent, PoeCompletedEvent, PoeErrorEvent,
    // RPC types
    PoeRunParams, PoeConfigParams, PoeRunResult,
    PoeStatusParams, PoeStatusResult,
    PoeCancelParams, PoeCancelResult,
    // Factories
    WorkerFactory, ValidatorFactory,
};

// Service layer
pub use services::{PoeRunManager, PoeContractService, PrepareParams, PrepareContext, RejectParams};

// Domain events
pub use event_bus::PoeEventBus;
pub use events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind, EventTier};

// Projectors
pub use projectors::memory::{MemoryFactWriter, MemoryProjector, NoOpMemoryFactWriter};
pub use projectors::runner::{ProjectorHandler, ProjectorRunner};
pub use projectors::trust::TrustProjector;
