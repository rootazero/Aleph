//! Dispatcher Layer - Core Scheduling Center
//!
//! This module is Aleph's core dispatch center, responsible for:
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
//! use alephcore::dispatcher::{
//!     ToolRegistry, UnifiedTool, ToolSource,
//!     AgentEngine, AgentConfig, ModelRouter
//! };
//!
//! // Create tool registry
//! let registry = ToolRegistry::new();
//! registry.refresh_all().await;
//!
//! // Create agent engine for task orchestration
//! let engine = AgentEngine::new(config, provider);
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
pub mod agent_types;
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


// === Experience Replay Layer: L1.5 routing ===
pub mod experience_replay_layer;

// === Tool Index: Semantic tool retrieval ===
pub mod tool_index;

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
pub use registry::ResolvedCommand;
pub use types::{
    ChannelType, ConflictInfo, ConflictResolution, DispatchMode, RoutingLayer, StructuredToolMeta,
    ToolCategory, ToolDefinition, ToolDiff, ToolIndex, ToolIndexCategory, ToolIndexEntry,
    ToolPriority, ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool,
    UnifiedToolInfo,
};
// Note: types::Capability is NOT re-exported here to avoid conflict with model_router::Capability
// Use types::Capability directly if needed for structured tool descriptions

// === Re-exports: Task Orchestration ===
pub use callback::{
    DagTaskDisplayStatus, DagTaskInfo, DagTaskPlan, ExecutionCallback, NoOpExecutionCallback,
    UserDecision,
};
pub use agent_types::{
    AiTask, AppAuto, AudioGenTask, CodeExec, CollaborativeStage, CollaborativeTask, DocGen,
    ExecutionSummary, FileOp, GraphValidationError, ImageGenTask, Language, Task,
    TaskCountByStatus, TaskDependency, TaskGraph, TaskGraphMeta,
    TaskResult as CoworkTaskResult, TaskStatus, TaskType, VideoGenTask,
};
pub use engine::{
    AgentConfig, AgentEngine, ExecutionState, DEFAULT_ALLOW_NETWORK, DEFAULT_CODE_EXEC_ENABLED,
    DEFAULT_CODE_EXEC_RUNTIME, DEFAULT_CODE_EXEC_TIMEOUT, DEFAULT_CONFIRMATION_TIMEOUT_SECS,
    DEFAULT_CONNECTION_TIMEOUT_SECS, DEFAULT_FILE_OPS_ENABLED, DEFAULT_MAX_FILE_SIZE,
    DEFAULT_MAX_RETRIES, DEFAULT_MAX_TOKENS, DEFAULT_PASS_ENV,
    DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE, DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE,
    DEFAULT_SANDBOX_ENABLED, MAX_PARALLELISM, MAX_STDERR_SIZE, MAX_STDOUT_SIZE, MAX_TASK_RETRIES,
    REQUIRE_CONFIRMATION,
};
pub use executor::{
    CollaborativeExecutor, ExecutionContext, ExecutorRegistry, NoopExecutor, TaskExecutor,
};
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


// === Re-exports: Experience Replay Layer ===
pub use experience_replay_layer::{ExperienceReplayConfig, ExperienceReplayLayer};

// === Re-exports: Tool Index (Semantic Retrieval) ===
pub use tool_index::{
    HydrationLevel, HydrationPipeline, HydrationPipelineConfig, HydrationResult,
    HydratedTool, InferredPurpose, SemanticPurposeInferrer, ToolIndexCoordinator,
    ToolMeta, ToolRetrieval, ToolRetrievalConfig,
};

#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;

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
