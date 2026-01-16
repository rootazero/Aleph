//! Cowork Task Orchestration Module
//!
//! This module provides a local-first AI task orchestration system,
//! enabling complex multi-step task execution with LLM-driven planning,
//! DAG-based scheduling, and extensible executors.
//!
//! # Architecture
//!
//! ```text
//! User Request
//!     │
//!     ▼
//! ┌─────────────┐
//! │   Planner   │ ──▶ TaskGraph (DAG)
//! └─────────────┘
//!     │
//!     ▼
//! ┌─────────────┐
//! │  Scheduler  │ ──▶ Ready Tasks
//! └─────────────┘
//!     │
//!     ▼
//! ┌─────────────┐
//! │  Executors  │ ──▶ TaskResult
//! └─────────────┘
//!     │
//!     ▼
//! ┌─────────────┐
//! │   Monitor   │ ──▶ ProgressEvent
//! └─────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::cowork::{CoworkEngine, CoworkConfig};
//!
//! // Create engine with provider
//! let engine = CoworkEngine::new(config, provider);
//!
//! // Plan a task
//! let graph = engine.plan("Organize my downloads folder").await?;
//!
//! // Execute with progress tracking
//! let summary = engine.execute(graph).await?;
//! ```

pub mod executor;
pub mod model_router;
pub mod monitor;
pub mod planner;
pub mod scheduler;
pub mod types;

mod engine;

// Re-export main types
pub use engine::{CoworkConfig, CoworkEngine, ExecutionState};
pub use executor::{ExecutionContext, ExecutorRegistry, NoopExecutor, TaskExecutor};
pub use monitor::{ProgressEvent, ProgressMonitor, ProgressSubscriber, TaskMonitor};
pub use planner::{LlmTaskPlanner, TaskPlanner};
pub use scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};
pub use types::{
    AiTask, AppAuto, CodeExec, DocGen, ExecutionSummary, FileOp, GraphValidationError, Language,
    Task, TaskCountByStatus, TaskDependency, TaskGraph, TaskGraphMeta, TaskResult, TaskStatus,
    TaskType,
};
pub use model_router::{
    Capability, ContextError, ContextSummary, CostStrategy, CostTier, ExecutionResult, LatencyTier,
    ModelMatcher, ModelProfile, ModelRouter, ModelRoutingRules, PipelineContext, PipelineError,
    PipelineEvent, PipelineExecutor, PipelineProgressHandler, PipelineStage, PipelineState,
    PipelineSummary, ProviderAdapter, RoutingError, StageResult, StoredTaskResult, TaskContext,
    TaskContextManager, TaskResultMetadata,
};
