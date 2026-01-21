//! Dispatcher Layer - Core Scheduling Center
//!
//! This module is Aether's core dispatch center, responsible for:
//!
//! - **Model Routing**: Intelligent model selection based on task characteristics
//! - **Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Task Orchestration**: DAG-based task scheduling and execution
//! - **Confirmation System**: User confirmation for tool execution
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Dispatcher Layer                         │
//! │                                                              │
//! │  ┌───────────────┐    ┌───────────────┐    ┌─────────────┐  │
//! │  │ ToolRegistry  │    │  ModelRouter  │    │  Cowork     │  │
//! │  └───────┬───────┘    └───────┬───────┘    │  Engine     │  │
//! │          │                    │            └──────┬──────┘  │
//! │          ▼                    ▼                   ▼         │
//! │  ┌───────────────┐    ┌───────────────┐    ┌─────────────┐  │
//! │  │ Confirmation  │    │   Executors   │    │  Scheduler  │  │
//! │  └───────────────┘    └───────────────┘    └─────────────┘  │
//! └──────────────────────────────────────────────────────────────┘
//!            ↓
//!     rig-core Agent
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{
//!     ToolRegistry, UnifiedTool, ToolSource,
//!     CoworkEngine, CoworkConfig, ModelRouter
//! };
//!
//! // Create tool registry
//! let registry = ToolRegistry::new();
//! registry.refresh_all().await;
//!
//! // Create cowork engine for task orchestration
//! let engine = CoworkEngine::new(config, provider);
//! let graph = engine.plan("Organize my downloads folder").await?;
//! let summary = engine.execute(graph).await?;
//! ```

// === Tool Management ===
mod async_confirmation;
mod confirmation;
mod integration;
mod registry;
mod types;

// === Task Orchestration (formerly cowork) ===
pub mod callback;
pub mod cowork_types;
pub mod executor;
pub mod model_router;
pub mod monitor;
pub mod planner;
pub mod scheduler;

mod engine;

// === Task Context ===
pub mod context;

// === Task Analysis ===
pub mod analyzer;

// === Risk Evaluation ===
pub mod risk;

// === Re-exports: Tool Management ===
pub use async_confirmation::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationState, PendingConfirmation,
    PendingConfirmationInfo, PendingConfirmationStore, UserConfirmationDecision,
};
pub use confirmation::{
    ConfirmationAction, ConfirmationConfig, ConfirmationDecision, ToolConfirmation, OPTION_CANCEL,
    OPTION_EDIT, OPTION_EXECUTE,
};
pub use integration::{
    ConfidenceAction, ConfidenceThresholds, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult,
};
pub use registry::ToolRegistry;
pub use types::{
    ConflictInfo, ConflictResolution, RoutingLayer, ToolCategory, ToolDefinition, ToolPriority,
    ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

// === Re-exports: Task Orchestration ===
pub use callback::{
    DagTaskDisplayStatus, DagTaskInfo, DagTaskPlan, ExecutionCallback, NoOpCallback, UserDecision,
};
pub use cowork_types::{
    AiTask, AppAuto, AudioGenTask, CodeExec, DocGen, ExecutionSummary, FileOp,
    GraphValidationError, ImageGenTask, Language, Task, TaskCountByStatus, TaskDependency,
    TaskGraph, TaskGraphMeta, TaskResult as CoworkTaskResult, TaskStatus, TaskType, VideoGenTask,
};
pub use engine::{CoworkConfig, CoworkEngine, ExecutionState};
pub use executor::{ExecutionContext, ExecutorRegistry, NoopExecutor, TaskExecutor};
pub use model_router::{
    Capability, CostStrategy, CostTier, FallbackProvider, LatencyTier, ModelMatcher, ModelProfile,
    ModelRouter, ModelRoutingRules, RoutingError, StageResult, TaskContextManager, TaskIntent,
};
pub use monitor::{ProgressEvent, ProgressMonitor, ProgressSubscriber, TaskMonitor};
pub use planner::{GenerationProviders, LlmTaskPlanner, TaskPlanner};
pub use scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};

// === Re-exports: Task Context ===
pub use context::{OutputType, TaskContext, TaskOutput};

// === Re-exports: Task Analysis ===
pub use analyzer::{AnalysisResult, TaskAnalyzer};

// === Re-exports: Risk Evaluation ===
pub use risk::{RiskEvaluator, RiskLevel};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_source_display() {
        assert_eq!(format!("{:?}", ToolSource::Native), "Native");
        assert_eq!(
            format!(
                "{:?}",
                ToolSource::Mcp {
                    server: "github".into()
                }
            ),
            "Mcp { server: \"github\" }"
        );
    }
}
