//! Cowork FFI bindings
//!
//! This module provides FFI-safe wrapper types for the Cowork task orchestration system.
//! These types are designed to work with UniFFI for Swift/Kotlin interop.

use std::sync::Arc;

use crate::cowork::{CoworkConfig, ExecutionState};
use crate::cowork::model_router::{
    Capability, CostStrategy, CostTier, LatencyTier, ModelProfile, ModelRoutingRules,
    StageResult,
};
use crate::cowork::monitor::{ProgressEvent, ProgressSubscriber};
use crate::cowork::types::{
    ExecutionSummary, Task, TaskDependency, TaskGraph, TaskStatus, TaskType,
};

// ============================================================================
// FFI Enums
// ============================================================================

/// Execution state for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkExecutionState {
    Idle,
    Planning,
    AwaitingConfirmation,
    Executing,
    Paused,
    Cancelled,
    Completed,
}

impl From<ExecutionState> for CoworkExecutionState {
    fn from(state: ExecutionState) -> Self {
        match state {
            ExecutionState::Idle => CoworkExecutionState::Idle,
            ExecutionState::Planning => CoworkExecutionState::Planning,
            ExecutionState::AwaitingConfirmation => CoworkExecutionState::AwaitingConfirmation,
            ExecutionState::Executing => CoworkExecutionState::Executing,
            ExecutionState::Paused => CoworkExecutionState::Paused,
            ExecutionState::Cancelled => CoworkExecutionState::Cancelled,
            ExecutionState::Completed => CoworkExecutionState::Completed,
        }
    }
}

/// Task status state for FFI (simplified)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkTaskStatusState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl From<&TaskStatus> for CoworkTaskStatusState {
    fn from(status: &TaskStatus) -> Self {
        match status {
            TaskStatus::Pending => CoworkTaskStatusState::Pending,
            TaskStatus::Running { .. } => CoworkTaskStatusState::Running,
            TaskStatus::Completed { .. } => CoworkTaskStatusState::Completed,
            TaskStatus::Failed { .. } => CoworkTaskStatusState::Failed,
            TaskStatus::Cancelled => CoworkTaskStatusState::Cancelled,
        }
    }
}

/// Task type category for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkTaskTypeCategory {
    FileOperation,
    CodeExecution,
    DocumentGeneration,
    AppAutomation,
    AiInference,
}

impl From<&TaskType> for CoworkTaskTypeCategory {
    fn from(task_type: &TaskType) -> Self {
        match task_type {
            TaskType::FileOperation(_) => CoworkTaskTypeCategory::FileOperation,
            TaskType::CodeExecution(_) => CoworkTaskTypeCategory::CodeExecution,
            TaskType::DocumentGeneration(_) => CoworkTaskTypeCategory::DocumentGeneration,
            TaskType::AppAutomation(_) => CoworkTaskTypeCategory::AppAutomation,
            TaskType::AiInference(_) => CoworkTaskTypeCategory::AiInference,
        }
    }
}

/// Progress event type for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoworkProgressEventType {
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    GraphProgress,
    GraphCompleted,
}

// ============================================================================
// Model Router FFI Enums
// ============================================================================

/// Model capability for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCapabilityFFI {
    CodeGeneration,
    CodeReview,
    TextAnalysis,
    ImageUnderstanding,
    VideoUnderstanding,
    LongContext,
    Reasoning,
    LocalPrivacy,
    FastResponse,
    SimpleTask,
    LongDocument,
}

impl From<Capability> for ModelCapabilityFFI {
    fn from(cap: Capability) -> Self {
        match cap {
            Capability::CodeGeneration => ModelCapabilityFFI::CodeGeneration,
            Capability::CodeReview => ModelCapabilityFFI::CodeReview,
            Capability::TextAnalysis => ModelCapabilityFFI::TextAnalysis,
            Capability::ImageUnderstanding => ModelCapabilityFFI::ImageUnderstanding,
            Capability::VideoUnderstanding => ModelCapabilityFFI::VideoUnderstanding,
            Capability::LongContext => ModelCapabilityFFI::LongContext,
            Capability::Reasoning => ModelCapabilityFFI::Reasoning,
            Capability::LocalPrivacy => ModelCapabilityFFI::LocalPrivacy,
            Capability::FastResponse => ModelCapabilityFFI::FastResponse,
            Capability::SimpleTask => ModelCapabilityFFI::SimpleTask,
            Capability::LongDocument => ModelCapabilityFFI::LongDocument,
        }
    }
}

impl From<ModelCapabilityFFI> for Capability {
    fn from(cap: ModelCapabilityFFI) -> Self {
        match cap {
            ModelCapabilityFFI::CodeGeneration => Capability::CodeGeneration,
            ModelCapabilityFFI::CodeReview => Capability::CodeReview,
            ModelCapabilityFFI::TextAnalysis => Capability::TextAnalysis,
            ModelCapabilityFFI::ImageUnderstanding => Capability::ImageUnderstanding,
            ModelCapabilityFFI::VideoUnderstanding => Capability::VideoUnderstanding,
            ModelCapabilityFFI::LongContext => Capability::LongContext,
            ModelCapabilityFFI::Reasoning => Capability::Reasoning,
            ModelCapabilityFFI::LocalPrivacy => Capability::LocalPrivacy,
            ModelCapabilityFFI::FastResponse => Capability::FastResponse,
            ModelCapabilityFFI::SimpleTask => Capability::SimpleTask,
            ModelCapabilityFFI::LongDocument => Capability::LongDocument,
        }
    }
}

/// Model cost tier for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCostTierFFI {
    Free,
    Low,
    Medium,
    High,
}

impl From<CostTier> for ModelCostTierFFI {
    fn from(tier: CostTier) -> Self {
        match tier {
            CostTier::Free => ModelCostTierFFI::Free,
            CostTier::Low => ModelCostTierFFI::Low,
            CostTier::Medium => ModelCostTierFFI::Medium,
            CostTier::High => ModelCostTierFFI::High,
        }
    }
}

impl From<ModelCostTierFFI> for CostTier {
    fn from(tier: ModelCostTierFFI) -> Self {
        match tier {
            ModelCostTierFFI::Free => CostTier::Free,
            ModelCostTierFFI::Low => CostTier::Low,
            ModelCostTierFFI::Medium => CostTier::Medium,
            ModelCostTierFFI::High => CostTier::High,
        }
    }
}

/// Model latency tier for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLatencyTierFFI {
    Fast,
    Medium,
    Slow,
}

impl From<LatencyTier> for ModelLatencyTierFFI {
    fn from(tier: LatencyTier) -> Self {
        match tier {
            LatencyTier::Fast => ModelLatencyTierFFI::Fast,
            LatencyTier::Medium => ModelLatencyTierFFI::Medium,
            LatencyTier::Slow => ModelLatencyTierFFI::Slow,
        }
    }
}

impl From<ModelLatencyTierFFI> for LatencyTier {
    fn from(tier: ModelLatencyTierFFI) -> Self {
        match tier {
            ModelLatencyTierFFI::Fast => LatencyTier::Fast,
            ModelLatencyTierFFI::Medium => LatencyTier::Medium,
            ModelLatencyTierFFI::Slow => LatencyTier::Slow,
        }
    }
}

/// Model cost strategy for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCostStrategyFFI {
    Cheapest,
    Balanced,
    BestQuality,
}

impl From<CostStrategy> for ModelCostStrategyFFI {
    fn from(strategy: CostStrategy) -> Self {
        match strategy {
            CostStrategy::Cheapest => ModelCostStrategyFFI::Cheapest,
            CostStrategy::Balanced => ModelCostStrategyFFI::Balanced,
            CostStrategy::BestQuality => ModelCostStrategyFFI::BestQuality,
        }
    }
}

impl From<ModelCostStrategyFFI> for CostStrategy {
    fn from(strategy: ModelCostStrategyFFI) -> Self {
        match strategy {
            ModelCostStrategyFFI::Cheapest => CostStrategy::Cheapest,
            ModelCostStrategyFFI::Balanced => CostStrategy::Balanced,
            ModelCostStrategyFFI::BestQuality => CostStrategy::BestQuality,
        }
    }
}

// ============================================================================
// FFI Dictionaries (Structs)
// ============================================================================

/// Cowork configuration for FFI
#[derive(Debug, Clone)]
pub struct CoworkConfigFFI {
    pub enabled: bool,
    pub require_confirmation: bool,
    pub max_parallelism: u32,
    pub dry_run: bool,
}

impl From<CoworkConfig> for CoworkConfigFFI {
    fn from(config: CoworkConfig) -> Self {
        Self {
            enabled: config.enabled,
            require_confirmation: config.require_confirmation,
            max_parallelism: config.max_parallelism as u32,
            dry_run: config.dry_run,
        }
    }
}

impl From<CoworkConfigFFI> for CoworkConfig {
    fn from(config: CoworkConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            require_confirmation: config.require_confirmation,
            max_parallelism: config.max_parallelism as usize,
            dry_run: config.dry_run,
            enable_pipelines: false,
            model_profiles: Vec::new(),
            routing_rules: None,
        }
    }
}

/// Code execution configuration for FFI
#[derive(Debug, Clone)]
pub struct CodeExecConfigFFI {
    /// Enable code execution (disabled by default for security)
    pub enabled: bool,
    /// Default runtime (shell, python, node)
    pub default_runtime: String,
    /// Execution timeout in seconds
    pub timeout_seconds: u64,
    /// Enable sandboxed execution
    pub sandbox_enabled: bool,
    /// Allow network access in sandbox
    pub allow_network: bool,
    /// Allowed runtimes (empty = all)
    pub allowed_runtimes: Vec<String>,
    /// Working directory for executions
    pub working_directory: Option<String>,
    /// Environment variables to pass
    pub pass_env: Vec<String>,
    /// Blocked command patterns
    pub blocked_commands: Vec<String>,
}

impl Default for CodeExecConfigFFI {
    fn default() -> Self {
        Self {
            enabled: false,
            default_runtime: "shell".to_string(),
            timeout_seconds: 60,
            sandbox_enabled: true,
            allow_network: false,
            allowed_runtimes: Vec::new(),
            working_directory: None,
            pass_env: vec!["PATH".to_string(), "HOME".to_string(), "USER".to_string()],
            blocked_commands: Vec::new(),
        }
    }
}

impl From<crate::config::types::cowork::CodeExecConfigToml> for CodeExecConfigFFI {
    fn from(config: crate::config::types::cowork::CodeExecConfigToml) -> Self {
        Self {
            enabled: config.enabled,
            default_runtime: config.default_runtime,
            timeout_seconds: config.timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
            allow_network: config.allow_network,
            allowed_runtimes: config.allowed_runtimes,
            working_directory: config.working_directory,
            pass_env: config.pass_env,
            blocked_commands: config.blocked_commands,
        }
    }
}

impl From<CodeExecConfigFFI> for crate::config::types::cowork::CodeExecConfigToml {
    fn from(config: CodeExecConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            default_runtime: config.default_runtime,
            timeout_seconds: config.timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
            allow_network: config.allow_network,
            allowed_runtimes: config.allowed_runtimes,
            working_directory: config.working_directory,
            pass_env: config.pass_env,
            blocked_commands: config.blocked_commands,
        }
    }
}

/// File operations configuration for FFI
#[derive(Debug, Clone)]
pub struct FileOpsConfigFFI {
    /// Enable file operations executor
    pub enabled: bool,
    /// Paths that are allowed for file operations (glob patterns)
    pub allowed_paths: Vec<String>,
    /// Paths that are denied for file operations (glob patterns)
    pub denied_paths: Vec<String>,
    /// Maximum file size in bytes for read operations
    pub max_file_size: u64,
    /// Require confirmation before write operations
    pub require_confirmation_for_write: bool,
    /// Require confirmation before delete operations
    pub require_confirmation_for_delete: bool,
}

impl Default for FileOpsConfigFFI {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            max_file_size: 100 * 1024 * 1024, // 100MB
            require_confirmation_for_write: true,
            require_confirmation_for_delete: true,
        }
    }
}

impl From<crate::config::types::cowork::FileOpsConfigToml> for FileOpsConfigFFI {
    fn from(config: crate::config::types::cowork::FileOpsConfigToml) -> Self {
        Self {
            enabled: config.enabled,
            allowed_paths: config.allowed_paths,
            denied_paths: config.denied_paths,
            max_file_size: config.max_file_size,
            require_confirmation_for_write: config.require_confirmation_for_write,
            require_confirmation_for_delete: config.require_confirmation_for_delete,
        }
    }
}

impl From<FileOpsConfigFFI> for crate::config::types::cowork::FileOpsConfigToml {
    fn from(config: FileOpsConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            allowed_paths: config.allowed_paths,
            denied_paths: config.denied_paths,
            max_file_size: config.max_file_size,
            require_confirmation_for_write: config.require_confirmation_for_write,
            require_confirmation_for_delete: config.require_confirmation_for_delete,
        }
    }
}

// ============================================================================
// Model Router FFI Structs
// ============================================================================

/// Model profile for FFI
#[derive(Debug, Clone)]
pub struct ModelProfileFFI {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub capabilities: Vec<ModelCapabilityFFI>,
    pub cost_tier: ModelCostTierFFI,
    pub latency_tier: ModelLatencyTierFFI,
    pub max_context: Option<u32>,
    pub local: bool,
}

impl From<ModelProfile> for ModelProfileFFI {
    fn from(profile: ModelProfile) -> Self {
        Self {
            id: profile.id,
            provider: profile.provider,
            model: profile.model,
            capabilities: profile.capabilities.into_iter().map(ModelCapabilityFFI::from).collect(),
            cost_tier: ModelCostTierFFI::from(profile.cost_tier),
            latency_tier: ModelLatencyTierFFI::from(profile.latency_tier),
            max_context: profile.max_context,
            local: profile.local,
        }
    }
}

impl From<&ModelProfile> for ModelProfileFFI {
    fn from(profile: &ModelProfile) -> Self {
        Self {
            id: profile.id.clone(),
            provider: profile.provider.clone(),
            model: profile.model.clone(),
            capabilities: profile.capabilities.iter().copied().map(ModelCapabilityFFI::from).collect(),
            cost_tier: ModelCostTierFFI::from(profile.cost_tier),
            latency_tier: ModelLatencyTierFFI::from(profile.latency_tier),
            max_context: profile.max_context,
            local: profile.local,
        }
    }
}

impl From<ModelProfileFFI> for ModelProfile {
    fn from(profile: ModelProfileFFI) -> Self {
        Self {
            id: profile.id,
            provider: profile.provider,
            model: profile.model,
            capabilities: profile.capabilities.into_iter().map(Capability::from).collect(),
            cost_tier: CostTier::from(profile.cost_tier),
            latency_tier: LatencyTier::from(profile.latency_tier),
            max_context: profile.max_context,
            local: profile.local,
            parameters: None,
        }
    }
}

/// Task type to model mapping entry for FFI
#[derive(Debug, Clone)]
pub struct TaskTypeMappingFFI {
    pub task_type: String,
    pub model_id: String,
}

/// Capability to model mapping entry for FFI
#[derive(Debug, Clone)]
pub struct CapabilityMappingFFI {
    pub capability: ModelCapabilityFFI,
    pub model_id: String,
}

/// Model routing rules for FFI
#[derive(Debug, Clone)]
pub struct ModelRoutingRulesFFI {
    pub task_type_mappings: Vec<TaskTypeMappingFFI>,
    pub capability_mappings: Vec<CapabilityMappingFFI>,
    pub cost_strategy: ModelCostStrategyFFI,
    pub default_model: Option<String>,
    pub enable_pipelines: bool,
}

impl From<ModelRoutingRules> for ModelRoutingRulesFFI {
    fn from(rules: ModelRoutingRules) -> Self {
        Self {
            task_type_mappings: rules.task_type_mappings
                .into_iter()
                .map(|(task_type, model_id)| TaskTypeMappingFFI { task_type, model_id })
                .collect(),
            capability_mappings: rules.capability_mappings
                .into_iter()
                .map(|(cap, model_id)| CapabilityMappingFFI {
                    capability: ModelCapabilityFFI::from(cap),
                    model_id,
                })
                .collect(),
            cost_strategy: ModelCostStrategyFFI::from(rules.cost_strategy),
            default_model: rules.default_model,
            enable_pipelines: rules.enable_pipelines,
        }
    }
}

impl From<&ModelRoutingRules> for ModelRoutingRulesFFI {
    fn from(rules: &ModelRoutingRules) -> Self {
        Self {
            task_type_mappings: rules.task_type_mappings
                .iter()
                .map(|(task_type, model_id)| TaskTypeMappingFFI {
                    task_type: task_type.clone(),
                    model_id: model_id.clone(),
                })
                .collect(),
            capability_mappings: rules.capability_mappings
                .iter()
                .map(|(cap, model_id)| CapabilityMappingFFI {
                    capability: ModelCapabilityFFI::from(*cap),
                    model_id: model_id.clone(),
                })
                .collect(),
            cost_strategy: ModelCostStrategyFFI::from(rules.cost_strategy),
            default_model: rules.default_model.clone(),
            enable_pipelines: rules.enable_pipelines,
        }
    }
}

impl From<ModelRoutingRulesFFI> for ModelRoutingRules {
    fn from(rules: ModelRoutingRulesFFI) -> Self {
        let mut result = ModelRoutingRules::default();

        for mapping in rules.task_type_mappings {
            result.task_type_mappings.insert(mapping.task_type, mapping.model_id);
        }

        for mapping in rules.capability_mappings {
            result.capability_mappings.insert(
                Capability::from(mapping.capability),
                mapping.model_id,
            );
        }

        result.cost_strategy = CostStrategy::from(rules.cost_strategy);
        result.default_model = rules.default_model;
        result.enable_pipelines = rules.enable_pipelines;

        result
    }
}

/// Stage result for FFI
#[derive(Debug, Clone)]
pub struct StageResultFFI {
    pub stage_id: String,
    pub model_used: String,
    pub provider: String,
    pub output_json: String,
    pub tokens_used: u32,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

impl From<StageResult> for StageResultFFI {
    fn from(result: StageResult) -> Self {
        Self {
            stage_id: result.stage_id,
            model_used: result.model_used,
            provider: result.provider,
            output_json: result.output.to_string(),
            tokens_used: result.tokens_used,
            duration_ms: result.duration.as_millis() as u64,
            success: result.success,
            error: result.error,
        }
    }
}

impl From<&StageResult> for StageResultFFI {
    fn from(result: &StageResult) -> Self {
        Self {
            stage_id: result.stage_id.clone(),
            model_used: result.model_used.clone(),
            provider: result.provider.clone(),
            output_json: result.output.to_string(),
            tokens_used: result.tokens_used,
            duration_ms: result.duration.as_millis() as u64,
            success: result.success,
            error: result.error.clone(),
        }
    }
}

// ============================================================================
// Task FFI Structs
// ============================================================================

/// Cowork task for FFI (simplified)
#[derive(Debug, Clone)]
pub struct CoworkTaskFFI {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub task_type: CoworkTaskTypeCategory,
    pub status: CoworkTaskStatusState,
    pub progress: f32,
    pub error_message: Option<String>,
}

impl From<&Task> for CoworkTaskFFI {
    fn from(task: &Task) -> Self {
        let error_message = if let TaskStatus::Failed { error, .. } = &task.status {
            Some(error.clone())
        } else {
            None
        };

        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            description: task.description.clone(),
            task_type: CoworkTaskTypeCategory::from(&task.task_type),
            status: CoworkTaskStatusState::from(&task.status),
            progress: task.progress(),
            error_message,
        }
    }
}

/// Cowork task dependency for FFI
#[derive(Debug, Clone)]
pub struct CoworkTaskDependencyFFI {
    pub from_task_id: String,
    pub to_task_id: String,
}

impl From<&TaskDependency> for CoworkTaskDependencyFFI {
    fn from(dep: &TaskDependency) -> Self {
        Self {
            from_task_id: dep.from.clone(),
            to_task_id: dep.to.clone(),
        }
    }
}

/// Cowork task graph for FFI
#[derive(Debug, Clone)]
pub struct CoworkTaskGraphFFI {
    pub id: String,
    pub title: String,
    pub original_request: Option<String>,
    pub tasks: Vec<CoworkTaskFFI>,
    pub edges: Vec<CoworkTaskDependencyFFI>,
}

impl From<&TaskGraph> for CoworkTaskGraphFFI {
    fn from(graph: &TaskGraph) -> Self {
        Self {
            id: graph.id.clone(),
            title: graph.metadata.title.clone(),
            original_request: graph.metadata.original_request.clone(),
            tasks: graph.tasks.iter().map(CoworkTaskFFI::from).collect(),
            edges: graph.edges.iter().map(CoworkTaskDependencyFFI::from).collect(),
        }
    }
}

/// Cowork execution summary for FFI
#[derive(Debug, Clone)]
pub struct CoworkExecutionSummaryFFI {
    pub graph_id: String,
    pub total_tasks: u32,
    pub completed_tasks: u32,
    pub failed_tasks: u32,
    pub cancelled_tasks: u32,
    pub total_duration_ms: u64,
    pub errors: Vec<String>,
}

impl From<ExecutionSummary> for CoworkExecutionSummaryFFI {
    fn from(summary: ExecutionSummary) -> Self {
        Self {
            graph_id: summary.graph_id,
            total_tasks: summary.total_tasks as u32,
            completed_tasks: summary.completed_tasks as u32,
            failed_tasks: summary.failed_tasks as u32,
            cancelled_tasks: summary.cancelled_tasks as u32,
            total_duration_ms: summary.total_duration.as_millis() as u64,
            errors: summary.errors,
        }
    }
}

/// Cowork progress event for FFI
#[derive(Debug, Clone)]
pub struct CoworkProgressEventFFI {
    pub event_type: CoworkProgressEventType,
    pub task_id: Option<String>,
    pub task_name: Option<String>,
    pub progress: f32,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl From<&ProgressEvent> for CoworkProgressEventFFI {
    fn from(event: &ProgressEvent) -> Self {
        match event {
            ProgressEvent::TaskStarted { task_id, task_name } => Self {
                event_type: CoworkProgressEventType::TaskStarted,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: None,
            },
            ProgressEvent::Progress {
                task_id,
                progress,
                message,
            } => Self {
                event_type: CoworkProgressEventType::TaskProgress,
                task_id: Some(task_id.clone()),
                task_name: None,
                progress: *progress,
                message: message.clone(),
                error: None,
            },
            ProgressEvent::TaskCompleted {
                task_id,
                task_name,
                ..
            } => Self {
                event_type: CoworkProgressEventType::TaskCompleted,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 1.0,
                message: None,
                error: None,
            },
            ProgressEvent::TaskFailed {
                task_id,
                task_name,
                error,
            } => Self {
                event_type: CoworkProgressEventType::TaskFailed,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: Some(error.clone()),
            },
            ProgressEvent::TaskCancelled { task_id, task_name } => Self {
                event_type: CoworkProgressEventType::TaskCancelled,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: None,
            },
            ProgressEvent::GraphProgress {
                graph_id,
                overall_progress,
                running_tasks,
                pending_tasks,
            } => Self {
                event_type: CoworkProgressEventType::GraphProgress,
                task_id: Some(graph_id.clone()),
                task_name: None,
                progress: *overall_progress,
                message: Some(format!(
                    "Running: {}, Pending: {}",
                    running_tasks, pending_tasks
                )),
                error: None,
            },
            ProgressEvent::GraphCompleted {
                graph_id,
                total_tasks,
                completed_tasks,
                failed_tasks,
            } => Self {
                event_type: CoworkProgressEventType::GraphCompleted,
                task_id: Some(graph_id.clone()),
                task_name: None,
                progress: 1.0,
                message: Some(format!(
                    "Total: {}, Completed: {}, Failed: {}",
                    total_tasks, completed_tasks, failed_tasks
                )),
                error: None,
            },
        }
    }
}

// ============================================================================
// FFI Callback Interface
// ============================================================================

/// Progress handler callback interface for FFI
pub trait CoworkProgressHandler: Send + Sync {
    /// Called when a progress event occurs
    fn on_progress_event(&self, event: CoworkProgressEventFFI);
}

/// Adapter to bridge FFI callback to internal ProgressSubscriber
pub struct FfiProgressSubscriber {
    handler: Arc<dyn CoworkProgressHandler>,
}

impl FfiProgressSubscriber {
    /// Create a new FFI progress subscriber
    pub fn new(handler: Arc<dyn CoworkProgressHandler>) -> Self {
        Self { handler }
    }
}

impl ProgressSubscriber for FfiProgressSubscriber {
    fn on_event(&self, event: ProgressEvent) {
        let ffi_event = CoworkProgressEventFFI::from(&event);
        self.handler.on_progress_event(ffi_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{FileOp, TaskResult};
    use std::path::PathBuf;

    #[test]
    fn test_execution_state_conversion() {
        assert_eq!(
            CoworkExecutionState::from(ExecutionState::Idle),
            CoworkExecutionState::Idle
        );
        assert_eq!(
            CoworkExecutionState::from(ExecutionState::Executing),
            CoworkExecutionState::Executing
        );
    }

    #[test]
    fn test_task_status_conversion() {
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::Pending),
            CoworkTaskStatusState::Pending
        );
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::running(0.5)),
            CoworkTaskStatusState::Running
        );
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::completed(TaskResult::default())),
            CoworkTaskStatusState::Completed
        );
        assert_eq!(
            CoworkTaskStatusState::from(&TaskStatus::failed("error")),
            CoworkTaskStatusState::Failed
        );
    }

    #[test]
    fn test_config_conversion() {
        let config = CoworkConfig {
            enabled: true,
            require_confirmation: false,
            max_parallelism: 8,
            dry_run: true,
            ..Default::default()
        };

        let ffi_config = CoworkConfigFFI::from(config.clone());
        assert_eq!(ffi_config.enabled, true);
        assert_eq!(ffi_config.max_parallelism, 8);
        assert_eq!(ffi_config.dry_run, true);

        let converted_back = CoworkConfig::from(ffi_config);
        assert_eq!(converted_back.enabled, config.enabled);
        assert_eq!(converted_back.max_parallelism, config.max_parallelism);
    }

    #[test]
    fn test_task_conversion() {
        let task = Task::new(
            "task_1",
            "Test Task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
        .with_description("A test task");

        let ffi_task = CoworkTaskFFI::from(&task);
        assert_eq!(ffi_task.id, "task_1");
        assert_eq!(ffi_task.name, "Test Task");
        assert_eq!(ffi_task.description, Some("A test task".to_string()));
        assert_eq!(ffi_task.task_type, CoworkTaskTypeCategory::FileOperation);
        assert_eq!(ffi_task.status, CoworkTaskStatusState::Pending);
        assert_eq!(ffi_task.progress, 0.0);
    }

    #[test]
    fn test_graph_conversion() {
        let mut graph = TaskGraph::new("graph_1", "Test Graph");
        graph.metadata.original_request = Some("Do something".to_string());

        graph.add_task(Task::new(
            "task_1",
            "Task 1",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        ));
        graph.add_task(Task::new(
            "task_2",
            "Task 2",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        ));
        graph.add_dependency("task_1", "task_2");

        let ffi_graph = CoworkTaskGraphFFI::from(&graph);
        assert_eq!(ffi_graph.id, "graph_1");
        assert_eq!(ffi_graph.title, "Test Graph");
        assert_eq!(ffi_graph.original_request, Some("Do something".to_string()));
        assert_eq!(ffi_graph.tasks.len(), 2);
        assert_eq!(ffi_graph.edges.len(), 1);
        assert_eq!(ffi_graph.edges[0].from_task_id, "task_1");
        assert_eq!(ffi_graph.edges[0].to_task_id, "task_2");
    }

    #[test]
    fn test_progress_event_conversion() {
        let event = ProgressEvent::TaskStarted {
            task_id: "task_1".to_string(),
            task_name: "Test Task".to_string(),
        };

        let ffi_event = CoworkProgressEventFFI::from(&event);
        assert_eq!(ffi_event.event_type, CoworkProgressEventType::TaskStarted);
        assert_eq!(ffi_event.task_id, Some("task_1".to_string()));
        assert_eq!(ffi_event.task_name, Some("Test Task".to_string()));
    }

    // =========================================================================
    // Model Router FFI Tests
    // =========================================================================

    #[test]
    fn test_model_capability_ffi_conversion() {
        use crate::cowork::model_router::Capability;

        // Test all capability conversions
        let capabilities = vec![
            (Capability::CodeGeneration, ModelCapabilityFFI::CodeGeneration),
            (Capability::CodeReview, ModelCapabilityFFI::CodeReview),
            (Capability::TextAnalysis, ModelCapabilityFFI::TextAnalysis),
            (Capability::ImageUnderstanding, ModelCapabilityFFI::ImageUnderstanding),
            (Capability::VideoUnderstanding, ModelCapabilityFFI::VideoUnderstanding),
            (Capability::LongContext, ModelCapabilityFFI::LongContext),
            (Capability::Reasoning, ModelCapabilityFFI::Reasoning),
            (Capability::LocalPrivacy, ModelCapabilityFFI::LocalPrivacy),
            (Capability::FastResponse, ModelCapabilityFFI::FastResponse),
            (Capability::SimpleTask, ModelCapabilityFFI::SimpleTask),
            (Capability::LongDocument, ModelCapabilityFFI::LongDocument),
        ];

        for (cap, expected_ffi) in capabilities {
            // Test Capability -> ModelCapabilityFFI
            let ffi: ModelCapabilityFFI = cap.into();
            assert_eq!(ffi, expected_ffi);

            // Test ModelCapabilityFFI -> Capability (round-trip)
            let back: Capability = ffi.into();
            assert_eq!(back, cap);
        }
    }

    #[test]
    fn test_model_cost_tier_ffi_conversion() {
        use crate::cowork::model_router::CostTier;

        let tiers = vec![
            (CostTier::Free, ModelCostTierFFI::Free),
            (CostTier::Low, ModelCostTierFFI::Low),
            (CostTier::Medium, ModelCostTierFFI::Medium),
            (CostTier::High, ModelCostTierFFI::High),
        ];

        for (tier, expected_ffi) in tiers {
            let ffi: ModelCostTierFFI = tier.into();
            assert_eq!(ffi, expected_ffi);

            let back: CostTier = ffi.into();
            assert_eq!(back, tier);
        }
    }

    #[test]
    fn test_model_latency_tier_ffi_conversion() {
        use crate::cowork::model_router::LatencyTier;

        let tiers = vec![
            (LatencyTier::Fast, ModelLatencyTierFFI::Fast),
            (LatencyTier::Medium, ModelLatencyTierFFI::Medium),
            (LatencyTier::Slow, ModelLatencyTierFFI::Slow),
        ];

        for (tier, expected_ffi) in tiers {
            let ffi: ModelLatencyTierFFI = tier.into();
            assert_eq!(ffi, expected_ffi);

            let back: LatencyTier = ffi.into();
            assert_eq!(back, tier);
        }
    }

    #[test]
    fn test_model_cost_strategy_ffi_conversion() {
        use crate::cowork::model_router::CostStrategy;

        let strategies = vec![
            (CostStrategy::Cheapest, ModelCostStrategyFFI::Cheapest),
            (CostStrategy::Balanced, ModelCostStrategyFFI::Balanced),
            (CostStrategy::BestQuality, ModelCostStrategyFFI::BestQuality),
        ];

        for (strategy, expected_ffi) in strategies {
            let ffi: ModelCostStrategyFFI = strategy.into();
            assert_eq!(ffi, expected_ffi);

            let back: CostStrategy = ffi.into();
            assert_eq!(back, strategy);
        }
    }

    #[test]
    fn test_model_profile_ffi_conversion() {
        use crate::cowork::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

        let profile = ModelProfile {
            id: "claude-opus".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![Capability::Reasoning, Capability::CodeGeneration],
            cost_tier: CostTier::High,
            latency_tier: LatencyTier::Slow,
            max_context: Some(200000),
            local: false,
            parameters: None,
        };

        // Test ModelProfile -> ModelProfileFFI
        let ffi: ModelProfileFFI = profile.clone().into();
        assert_eq!(ffi.id, "claude-opus");
        assert_eq!(ffi.provider, "anthropic");
        assert_eq!(ffi.model, "claude-opus-4");
        assert_eq!(ffi.capabilities.len(), 2);
        assert_eq!(ffi.cost_tier, ModelCostTierFFI::High);
        assert_eq!(ffi.latency_tier, ModelLatencyTierFFI::Slow);
        assert_eq!(ffi.max_context, Some(200000));
        assert!(!ffi.local);

        // Test ModelProfileFFI -> ModelProfile (round-trip)
        let back: ModelProfile = ffi.into();
        assert_eq!(back.id, profile.id);
        assert_eq!(back.provider, profile.provider);
        assert_eq!(back.model, profile.model);
        assert_eq!(back.capabilities.len(), profile.capabilities.len());
        assert_eq!(back.cost_tier, profile.cost_tier);
        assert_eq!(back.latency_tier, profile.latency_tier);
        assert_eq!(back.max_context, profile.max_context);
        assert_eq!(back.local, profile.local);
    }

    #[test]
    fn test_model_profile_ffi_from_ref() {
        use crate::cowork::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

        let profile = ModelProfile {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            capabilities: vec![Capability::ImageUnderstanding],
            cost_tier: CostTier::Medium,
            latency_tier: LatencyTier::Medium,
            max_context: None,
            local: false,
            parameters: None,
        };

        // Test &ModelProfile -> ModelProfileFFI
        let ffi: ModelProfileFFI = (&profile).into();
        assert_eq!(ffi.id, "gpt-4o");
        assert_eq!(ffi.provider, "openai");
        assert_eq!(ffi.capabilities.len(), 1);
        assert_eq!(ffi.capabilities[0], ModelCapabilityFFI::ImageUnderstanding);
    }

    #[test]
    fn test_model_profile_ffi_local_model() {
        use crate::cowork::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

        let profile = ModelProfile {
            id: "ollama-llama".to_string(),
            provider: "ollama".to_string(),
            model: "llama3.2".to_string(),
            capabilities: vec![Capability::LocalPrivacy, Capability::FastResponse],
            cost_tier: CostTier::Free,
            latency_tier: LatencyTier::Fast,
            max_context: None,
            local: true,
            parameters: None,
        };

        let ffi: ModelProfileFFI = profile.into();
        assert!(ffi.local);
        assert_eq!(ffi.cost_tier, ModelCostTierFFI::Free);
        assert_eq!(ffi.latency_tier, ModelLatencyTierFFI::Fast);
    }

    #[test]
    fn test_task_type_mapping_ffi() {
        let mapping = TaskTypeMappingFFI {
            task_type: "code_generation".to_string(),
            model_id: "claude-opus".to_string(),
        };

        assert_eq!(mapping.task_type, "code_generation");
        assert_eq!(mapping.model_id, "claude-opus");
    }

    #[test]
    fn test_capability_mapping_ffi() {
        let mapping = CapabilityMappingFFI {
            capability: ModelCapabilityFFI::Reasoning,
            model_id: "claude-opus".to_string(),
        };

        assert_eq!(mapping.capability, ModelCapabilityFFI::Reasoning);
        assert_eq!(mapping.model_id, "claude-opus");
    }

    #[test]
    fn test_model_routing_rules_ffi_creation() {
        let rules = ModelRoutingRulesFFI {
            cost_strategy: ModelCostStrategyFFI::Balanced,
            default_model: Some("claude-sonnet".to_string()),
            enable_pipelines: true,
            task_type_mappings: vec![
                TaskTypeMappingFFI {
                    task_type: "code_generation".to_string(),
                    model_id: "claude-opus".to_string(),
                },
                TaskTypeMappingFFI {
                    task_type: "quick_tasks".to_string(),
                    model_id: "claude-haiku".to_string(),
                },
            ],
            capability_mappings: vec![
                CapabilityMappingFFI {
                    capability: ModelCapabilityFFI::Reasoning,
                    model_id: "claude-opus".to_string(),
                },
            ],
        };

        assert_eq!(rules.cost_strategy, ModelCostStrategyFFI::Balanced);
        assert_eq!(rules.default_model, Some("claude-sonnet".to_string()));
        assert!(rules.enable_pipelines);
        assert_eq!(rules.task_type_mappings.len(), 2);
        assert_eq!(rules.capability_mappings.len(), 1);
    }

    #[test]
    fn test_stage_result_ffi() {
        let result = StageResultFFI {
            stage_id: "stage_1".to_string(),
            model_used: "claude-opus".to_string(),
            provider: "anthropic".to_string(),
            output_json: r#"{"result": "Generated code..."}"#.to_string(),
            tokens_used: 1500,
            duration_ms: 2500,
            success: true,
            error: None,
        };

        assert_eq!(result.stage_id, "stage_1");
        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.model_used, "claude-opus");
        assert_eq!(result.provider, "anthropic");
        assert_eq!(result.tokens_used, 1500);
        assert_eq!(result.duration_ms, 2500);

        // Test failed result
        let failed_result = StageResultFFI {
            stage_id: "stage_2".to_string(),
            model_used: String::new(),
            provider: String::new(),
            output_json: String::new(),
            tokens_used: 0,
            duration_ms: 100,
            success: false,
            error: Some("API error".to_string()),
        };

        assert!(!failed_result.success);
        assert_eq!(failed_result.error, Some("API error".to_string()));
    }
}
