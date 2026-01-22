//! Dispatcher FFI Types
//!
//! This module provides FFI-safe wrapper types for the Dispatcher layer:
//! - Task orchestration (scheduler, executor, planner)
//! - Model routing (profiles, rules, health monitoring)
//! - Budget management
//! - A/B testing and ensemble
//!
//! These types are designed to work with UniFFI for Swift/Kotlin interop.

use std::sync::Arc;

use crate::dispatcher::agent_types::{
    ExecutionSummary, Task, TaskDependency, TaskGraph, TaskStatus, TaskType,
};
use crate::dispatcher::model_router::{
    BudgetEnforcement,
    BudgetLimit,
    BudgetPeriod,
    // Budget types
    BudgetScope,
    BudgetState,
    Capability,
    CostStrategy,
    CostTier,
    HealthStatistics,
    HealthStatus,
    LatencyTier,
    ModelHealthSummary,
    ModelProfile,
    ModelRoutingRules,
    StageResult,
};
use crate::dispatcher::monitor::{ProgressEvent, ProgressSubscriber};
use crate::dispatcher::{AgentConfig, ExecutionState};

// ============================================================================
// FFI Enums
// ============================================================================

/// Execution state for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExecutionState {
    Idle,
    Planning,
    AwaitingConfirmation,
    Executing,
    Paused,
    Cancelled,
    Completed,
}

impl From<ExecutionState> for AgentExecutionState {
    fn from(state: ExecutionState) -> Self {
        match state {
            ExecutionState::Idle => AgentExecutionState::Idle,
            ExecutionState::Planning => AgentExecutionState::Planning,
            ExecutionState::AwaitingConfirmation => AgentExecutionState::AwaitingConfirmation,
            ExecutionState::Executing => AgentExecutionState::Executing,
            ExecutionState::Paused => AgentExecutionState::Paused,
            ExecutionState::Cancelled => AgentExecutionState::Cancelled,
            ExecutionState::Completed => AgentExecutionState::Completed,
        }
    }
}

/// Task status state for FFI (simplified)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTaskStatusState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl From<&TaskStatus> for AgentTaskStatusState {
    fn from(status: &TaskStatus) -> Self {
        match status {
            TaskStatus::Pending => AgentTaskStatusState::Pending,
            TaskStatus::Running { .. } => AgentTaskStatusState::Running,
            TaskStatus::Completed { .. } => AgentTaskStatusState::Completed,
            TaskStatus::Failed { .. } => AgentTaskStatusState::Failed,
            TaskStatus::Cancelled => AgentTaskStatusState::Cancelled,
        }
    }
}

/// Task type category for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTaskTypeCategory {
    FileOperation,
    CodeExecution,
    DocumentGeneration,
    AppAutomation,
    AiInference,
    ImageGeneration,
    VideoGeneration,
    AudioGeneration,
}

impl From<&TaskType> for AgentTaskTypeCategory {
    fn from(task_type: &TaskType) -> Self {
        match task_type {
            TaskType::FileOperation(_) => AgentTaskTypeCategory::FileOperation,
            TaskType::CodeExecution(_) => AgentTaskTypeCategory::CodeExecution,
            TaskType::DocumentGeneration(_) => AgentTaskTypeCategory::DocumentGeneration,
            TaskType::AppAutomation(_) => AgentTaskTypeCategory::AppAutomation,
            TaskType::AiInference(_) => AgentTaskTypeCategory::AiInference,
            TaskType::ImageGeneration(_) => AgentTaskTypeCategory::ImageGeneration,
            TaskType::VideoGeneration(_) => AgentTaskTypeCategory::VideoGeneration,
            TaskType::AudioGeneration(_) => AgentTaskTypeCategory::AudioGeneration,
        }
    }
}

/// Progress event type for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentProgressEventType {
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
pub struct AgentConfigFFI {
    pub require_confirmation: bool,
    pub max_parallelism: u32,
    pub max_task_retries: u32,
    pub dry_run: bool,
}

impl From<AgentConfig> for AgentConfigFFI {
    fn from(config: AgentConfig) -> Self {
        Self {
            require_confirmation: config.require_confirmation,
            max_parallelism: config.max_parallelism as u32,
            max_task_retries: config.max_task_retries,
            dry_run: config.dry_run,
        }
    }
}

impl From<AgentConfigFFI> for AgentConfig {
    fn from(config: AgentConfigFFI) -> Self {
        Self {
            require_confirmation: config.require_confirmation,
            max_parallelism: config.max_parallelism as usize,
            max_task_retries: config.max_task_retries,
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

impl From<crate::config::types::agent::CodeExecConfigToml> for CodeExecConfigFFI {
    fn from(config: crate::config::types::agent::CodeExecConfigToml) -> Self {
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

impl From<CodeExecConfigFFI> for crate::config::types::agent::CodeExecConfigToml {
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

impl From<crate::config::types::agent::FileOpsConfigToml> for FileOpsConfigFFI {
    fn from(config: crate::config::types::agent::FileOpsConfigToml) -> Self {
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

impl From<FileOpsConfigFFI> for crate::config::types::agent::FileOpsConfigToml {
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
            capabilities: profile
                .capabilities
                .into_iter()
                .map(ModelCapabilityFFI::from)
                .collect(),
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
            capabilities: profile
                .capabilities
                .iter()
                .copied()
                .map(ModelCapabilityFFI::from)
                .collect(),
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
            capabilities: profile
                .capabilities
                .into_iter()
                .map(Capability::from)
                .collect(),
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
            task_type_mappings: rules
                .task_type_mappings
                .into_iter()
                .map(|(task_type, model_id)| TaskTypeMappingFFI {
                    task_type,
                    model_id,
                })
                .collect(),
            capability_mappings: rules
                .capability_mappings
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
            task_type_mappings: rules
                .task_type_mappings
                .iter()
                .map(|(task_type, model_id)| TaskTypeMappingFFI {
                    task_type: task_type.clone(),
                    model_id: model_id.clone(),
                })
                .collect(),
            capability_mappings: rules
                .capability_mappings
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
            result
                .task_type_mappings
                .insert(mapping.task_type, mapping.model_id);
        }

        for mapping in rules.capability_mappings {
            result
                .capability_mappings
                .insert(Capability::from(mapping.capability), mapping.model_id);
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
// Model Health Monitoring FFI Types
// ============================================================================

/// Health status of an AI model for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelHealthStatusFFI {
    Healthy,
    Degraded,
    Unhealthy,
    CircuitOpen,
    HalfOpen,
    Unknown,
}

impl From<HealthStatus> for ModelHealthStatusFFI {
    fn from(status: HealthStatus) -> Self {
        match status {
            HealthStatus::Healthy => ModelHealthStatusFFI::Healthy,
            HealthStatus::Degraded => ModelHealthStatusFFI::Degraded,
            HealthStatus::Unhealthy => ModelHealthStatusFFI::Unhealthy,
            HealthStatus::CircuitOpen => ModelHealthStatusFFI::CircuitOpen,
            HealthStatus::HalfOpen => ModelHealthStatusFFI::HalfOpen,
            HealthStatus::Unknown => ModelHealthStatusFFI::Unknown,
        }
    }
}

impl From<ModelHealthStatusFFI> for HealthStatus {
    fn from(status: ModelHealthStatusFFI) -> Self {
        match status {
            ModelHealthStatusFFI::Healthy => HealthStatus::Healthy,
            ModelHealthStatusFFI::Degraded => HealthStatus::Degraded,
            ModelHealthStatusFFI::Unhealthy => HealthStatus::Unhealthy,
            ModelHealthStatusFFI::CircuitOpen => HealthStatus::CircuitOpen,
            ModelHealthStatusFFI::HalfOpen => HealthStatus::HalfOpen,
            ModelHealthStatusFFI::Unknown => HealthStatus::Unknown,
        }
    }
}

/// Summarized health information for a single model (UI display)
#[derive(Debug, Clone)]
pub struct ModelHealthSummaryFFI {
    pub model_id: String,
    pub status: ModelHealthStatusFFI,
    pub status_text: String,
    pub status_emoji: String,
    pub reason: Option<String>,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
}

impl From<ModelHealthSummary> for ModelHealthSummaryFFI {
    fn from(summary: ModelHealthSummary) -> Self {
        Self {
            model_id: summary.model_id,
            status: ModelHealthStatusFFI::from(summary.status),
            status_text: summary.status_text,
            status_emoji: summary.status_emoji,
            reason: summary.reason,
            consecutive_successes: summary.consecutive_successes,
            consecutive_failures: summary.consecutive_failures,
        }
    }
}

impl From<&ModelHealthSummary> for ModelHealthSummaryFFI {
    fn from(summary: &ModelHealthSummary) -> Self {
        Self {
            model_id: summary.model_id.clone(),
            status: ModelHealthStatusFFI::from(summary.status),
            status_text: summary.status_text.clone(),
            status_emoji: summary.status_emoji.clone(),
            reason: summary.reason.clone(),
            consecutive_successes: summary.consecutive_successes,
            consecutive_failures: summary.consecutive_failures,
        }
    }
}

/// Overall health statistics for all tracked models
#[derive(Debug, Clone)]
pub struct HealthStatisticsFFI {
    pub total: u32,
    pub healthy: u32,
    pub degraded: u32,
    pub unhealthy: u32,
    pub circuit_open: u32,
    pub half_open: u32,
    pub unknown: u32,
    pub healthy_percent: f64,
}

impl From<HealthStatistics> for HealthStatisticsFFI {
    fn from(stats: HealthStatistics) -> Self {
        Self {
            total: stats.total as u32,
            healthy: stats.healthy as u32,
            degraded: stats.degraded as u32,
            unhealthy: stats.unhealthy as u32,
            circuit_open: stats.circuit_open as u32,
            half_open: stats.half_open as u32,
            unknown: stats.unknown as u32,
            healthy_percent: stats.healthy_percent(),
        }
    }
}

impl From<&HealthStatistics> for HealthStatisticsFFI {
    fn from(stats: &HealthStatistics) -> Self {
        Self {
            total: stats.total as u32,
            healthy: stats.healthy as u32,
            degraded: stats.degraded as u32,
            unhealthy: stats.unhealthy as u32,
            circuit_open: stats.circuit_open as u32,
            half_open: stats.half_open as u32,
            unknown: stats.unknown as u32,
            healthy_percent: stats.healthy_percent(),
        }
    }
}

// ============================================================================
// Budget FFI Types (Model Router P1)
// ============================================================================

/// Budget scope for FFI (Global, Project, Session, Model)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetScopeFFI {
    Global,
    Project { id: String },
    Session { id: String },
    Model { id: String },
}

impl From<&BudgetScope> for BudgetScopeFFI {
    fn from(scope: &BudgetScope) -> Self {
        match scope {
            BudgetScope::Global => BudgetScopeFFI::Global,
            BudgetScope::Project(id) => BudgetScopeFFI::Project { id: id.clone() },
            BudgetScope::Session(id) => BudgetScopeFFI::Session { id: id.clone() },
            BudgetScope::Model(id) => BudgetScopeFFI::Model { id: id.clone() },
        }
    }
}

impl From<BudgetScope> for BudgetScopeFFI {
    fn from(scope: BudgetScope) -> Self {
        BudgetScopeFFI::from(&scope)
    }
}

impl From<&BudgetScopeFFI> for BudgetScope {
    fn from(ffi: &BudgetScopeFFI) -> Self {
        match ffi {
            BudgetScopeFFI::Global => BudgetScope::Global,
            BudgetScopeFFI::Project { id } => BudgetScope::Project(id.clone()),
            BudgetScopeFFI::Session { id } => BudgetScope::Session(id.clone()),
            BudgetScopeFFI::Model { id } => BudgetScope::Model(id.clone()),
        }
    }
}

impl From<BudgetScopeFFI> for BudgetScope {
    fn from(ffi: BudgetScopeFFI) -> Self {
        BudgetScope::from(&ffi)
    }
}

/// Budget period for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetPeriodFFI {
    Lifetime,
    Daily,
    Weekly,
    Monthly,
}

impl From<&BudgetPeriod> for BudgetPeriodFFI {
    fn from(period: &BudgetPeriod) -> Self {
        match period {
            BudgetPeriod::Lifetime => BudgetPeriodFFI::Lifetime,
            BudgetPeriod::Daily { .. } => BudgetPeriodFFI::Daily,
            BudgetPeriod::Weekly { .. } => BudgetPeriodFFI::Weekly,
            BudgetPeriod::Monthly { .. } => BudgetPeriodFFI::Monthly,
        }
    }
}

impl From<BudgetPeriod> for BudgetPeriodFFI {
    fn from(period: BudgetPeriod) -> Self {
        BudgetPeriodFFI::from(&period)
    }
}

/// Budget enforcement action for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetEnforcementFFI {
    WarnOnly,
    SoftBlock,
    HardBlock,
}

impl From<BudgetEnforcement> for BudgetEnforcementFFI {
    fn from(enforcement: BudgetEnforcement) -> Self {
        match enforcement {
            BudgetEnforcement::WarnOnly => BudgetEnforcementFFI::WarnOnly,
            BudgetEnforcement::SoftBlock => BudgetEnforcementFFI::SoftBlock,
            BudgetEnforcement::HardBlock => BudgetEnforcementFFI::HardBlock,
        }
    }
}

impl From<BudgetEnforcementFFI> for BudgetEnforcement {
    fn from(ffi: BudgetEnforcementFFI) -> Self {
        match ffi {
            BudgetEnforcementFFI::WarnOnly => BudgetEnforcement::WarnOnly,
            BudgetEnforcementFFI::SoftBlock => BudgetEnforcement::SoftBlock,
            BudgetEnforcementFFI::HardBlock => BudgetEnforcement::HardBlock,
        }
    }
}

/// Status of a single budget limit for FFI
#[derive(Debug, Clone)]
pub struct BudgetLimitStatusFFI {
    /// Limit unique identifier
    pub limit_id: String,
    /// Scope this limit applies to
    pub scope: BudgetScopeFFI,
    /// Scope as display string
    pub scope_display: String,
    /// Budget period type
    pub period: BudgetPeriodFFI,
    /// Period as display string
    pub period_display: String,
    /// Configured limit in USD
    pub limit_usd: f64,
    /// Current spend in USD
    pub spent_usd: f64,
    /// Remaining budget in USD
    pub remaining_usd: f64,
    /// Percentage used (0.0 - 1.0)
    pub used_percent: f64,
    /// Enforcement action when exceeded
    pub enforcement: BudgetEnforcementFFI,
    /// Whether the limit is currently exceeded
    pub is_exceeded: bool,
    /// Whether any warning threshold has been crossed
    pub is_warning: bool,
    /// Next reset timestamp (Unix epoch seconds)
    pub next_reset_timestamp: i64,
    /// Human-readable time until reset
    pub next_reset_display: String,
}

impl BudgetLimitStatusFFI {
    /// Create from a BudgetLimit and BudgetState
    pub fn from_limit_and_state(limit: &BudgetLimit, state: &BudgetState) -> Self {
        let now = chrono::Utc::now();
        let duration_until_reset = state.next_reset.signed_duration_since(now);

        let next_reset_display = if duration_until_reset.num_hours() < 1 {
            format!("{} minutes", duration_until_reset.num_minutes().max(1))
        } else if duration_until_reset.num_days() < 1 {
            format!("{} hours", duration_until_reset.num_hours())
        } else {
            format!("{} days", duration_until_reset.num_days())
        };

        Self {
            limit_id: limit.id.clone(),
            scope: BudgetScopeFFI::from(&limit.scope),
            scope_display: limit.scope.as_str(),
            period: BudgetPeriodFFI::from(&limit.period),
            period_display: limit.period.as_str().to_string(),
            limit_usd: limit.limit_usd,
            spent_usd: state.spent_usd,
            remaining_usd: state.remaining_usd,
            used_percent: state.used_percent,
            enforcement: BudgetEnforcementFFI::from(limit.enforcement),
            is_exceeded: state.spent_usd >= limit.limit_usd,
            is_warning: !state.warnings_fired.is_empty(),
            next_reset_timestamp: state.next_reset.timestamp(),
            next_reset_display,
        }
    }
}

/// Overall budget status summary for FFI
#[derive(Debug, Clone)]
pub struct BudgetStatusFFI {
    /// Whether budget management is enabled
    pub enabled: bool,
    /// Total number of configured limits
    pub total_limits: u32,
    /// Number of limits currently exceeded
    pub exceeded_count: u32,
    /// Number of limits with active warnings
    pub warning_count: u32,
    /// Total spent across all scopes in USD
    pub total_spent_usd: f64,
    /// Total remaining across all limits in USD (minimum remaining)
    pub min_remaining_usd: f64,
    /// Status per configured limit
    pub limits: Vec<BudgetLimitStatusFFI>,
    /// Status emoji for quick display
    pub status_emoji: String,
    /// Human-readable status message
    pub status_message: String,
}

impl BudgetStatusFFI {
    /// Create a disabled budget status
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            total_limits: 0,
            exceeded_count: 0,
            warning_count: 0,
            total_spent_usd: 0.0,
            min_remaining_usd: f64::MAX,
            limits: Vec::new(),
            status_emoji: "⚫".to_string(),
            status_message: "Budget management disabled".to_string(),
        }
    }

    /// Create status from BudgetManager data
    pub fn from_limits_and_states(
        limits: &[BudgetLimit],
        states: &std::collections::HashMap<String, BudgetState>,
    ) -> Self {
        if limits.is_empty() {
            return Self::disabled();
        }

        let mut limit_statuses = Vec::new();
        let mut total_spent = 0.0;
        let mut min_remaining = f64::MAX;
        let mut exceeded_count = 0u32;
        let mut warning_count = 0u32;

        for limit in limits {
            if let Some(state) = states.get(&limit.id) {
                let status = BudgetLimitStatusFFI::from_limit_and_state(limit, state);

                total_spent += state.spent_usd;
                if status.remaining_usd < min_remaining {
                    min_remaining = status.remaining_usd;
                }
                if status.is_exceeded {
                    exceeded_count += 1;
                }
                if status.is_warning {
                    warning_count += 1;
                }

                limit_statuses.push(status);
            }
        }

        let (status_emoji, status_message) = if exceeded_count > 0 {
            (
                "🔴".to_string(),
                format!("{} budget(s) exceeded", exceeded_count),
            )
        } else if warning_count > 0 {
            (
                "🟡".to_string(),
                format!("{} budget warning(s)", warning_count),
            )
        } else {
            ("🟢".to_string(), "All budgets healthy".to_string())
        };

        Self {
            enabled: true,
            total_limits: limits.len() as u32,
            exceeded_count,
            warning_count,
            total_spent_usd: total_spent,
            min_remaining_usd: if min_remaining == f64::MAX {
                0.0
            } else {
                min_remaining
            },
            limits: limit_statuses,
            status_emoji,
            status_message,
        }
    }
}

// ============================================================================
// P2: Prompt Analysis FFI Types
// ============================================================================

/// Detected language for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageFFI {
    English,
    Chinese,
    Japanese,
    Korean,
    Mixed,
    Unknown,
}

impl From<crate::dispatcher::model_router::Language> for LanguageFFI {
    fn from(lang: crate::dispatcher::model_router::Language) -> Self {
        match lang {
            crate::dispatcher::model_router::Language::English => LanguageFFI::English,
            crate::dispatcher::model_router::Language::Chinese => LanguageFFI::Chinese,
            crate::dispatcher::model_router::Language::Japanese => LanguageFFI::Japanese,
            crate::dispatcher::model_router::Language::Korean => LanguageFFI::Korean,
            crate::dispatcher::model_router::Language::Mixed => LanguageFFI::Mixed,
            crate::dispatcher::model_router::Language::Unknown => LanguageFFI::Unknown,
        }
    }
}

/// Reasoning level for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningLevelFFI {
    Low,
    Medium,
    High,
}

impl From<crate::dispatcher::model_router::ReasoningLevel> for ReasoningLevelFFI {
    fn from(level: crate::dispatcher::model_router::ReasoningLevel) -> Self {
        match level {
            crate::dispatcher::model_router::ReasoningLevel::Low => ReasoningLevelFFI::Low,
            crate::dispatcher::model_router::ReasoningLevel::Medium => ReasoningLevelFFI::Medium,
            crate::dispatcher::model_router::ReasoningLevel::High => ReasoningLevelFFI::High,
        }
    }
}

/// Suggested context size for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSizeFFI {
    Small,
    Medium,
    Large,
}

impl From<crate::dispatcher::model_router::ContextSize> for ContextSizeFFI {
    fn from(size: crate::dispatcher::model_router::ContextSize) -> Self {
        match size {
            crate::dispatcher::model_router::ContextSize::Small => ContextSizeFFI::Small,
            crate::dispatcher::model_router::ContextSize::Medium => ContextSizeFFI::Medium,
            crate::dispatcher::model_router::ContextSize::Large => ContextSizeFFI::Large,
        }
    }
}

/// Detected domain for FFI (simplified)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainFFI {
    General,
    Creative,
    Conversational,
    TechnicalProgramming,
    TechnicalMathematics,
    TechnicalScience,
    TechnicalEngineering,
    TechnicalDataScience,
    TechnicalOther,
}

impl From<crate::dispatcher::model_router::Domain> for DomainFFI {
    fn from(domain: crate::dispatcher::model_router::Domain) -> Self {
        match domain {
            crate::dispatcher::model_router::Domain::General => DomainFFI::General,
            crate::dispatcher::model_router::Domain::Creative => DomainFFI::Creative,
            crate::dispatcher::model_router::Domain::Conversational => DomainFFI::Conversational,
            crate::dispatcher::model_router::Domain::Technical(tech) => match tech {
                crate::dispatcher::model_router::TechnicalDomain::Programming => {
                    DomainFFI::TechnicalProgramming
                }
                crate::dispatcher::model_router::TechnicalDomain::Mathematics => {
                    DomainFFI::TechnicalMathematics
                }
                crate::dispatcher::model_router::TechnicalDomain::Science => {
                    DomainFFI::TechnicalScience
                }
                crate::dispatcher::model_router::TechnicalDomain::Engineering => {
                    DomainFFI::TechnicalEngineering
                }
                crate::dispatcher::model_router::TechnicalDomain::DataScience => {
                    DomainFFI::TechnicalDataScience
                }
                crate::dispatcher::model_router::TechnicalDomain::Other(_) => {
                    DomainFFI::TechnicalOther
                }
            },
        }
    }
}

/// Prompt features extracted by PromptAnalyzer for FFI
#[derive(Debug, Clone)]
pub struct PromptFeaturesFFI {
    /// Estimated token count
    pub estimated_tokens: u32,
    /// Complexity score (0.0 - 1.0)
    pub complexity_score: f64,
    /// Primary detected language
    pub primary_language: LanguageFFI,
    /// Confidence in language detection (0.0 - 1.0)
    pub language_confidence: f64,
    /// Ratio of code content (0.0 - 1.0)
    pub code_ratio: f64,
    /// Detected reasoning level
    pub reasoning_level: ReasoningLevelFFI,
    /// Detected domain
    pub domain: DomainFFI,
    /// Suggested context size for model selection
    pub suggested_context_size: ContextSizeFFI,
    /// Analysis time in microseconds
    pub analysis_time_us: u64,
    /// Whether prompt contains code blocks
    pub has_code_blocks: bool,
    /// Number of questions detected
    pub question_count: u32,
    /// Number of imperative statements detected
    pub imperative_count: u32,
}

impl From<crate::dispatcher::model_router::PromptFeatures> for PromptFeaturesFFI {
    fn from(features: crate::dispatcher::model_router::PromptFeatures) -> Self {
        Self {
            estimated_tokens: features.estimated_tokens,
            complexity_score: features.complexity_score,
            primary_language: features.primary_language.into(),
            language_confidence: features.language_confidence,
            code_ratio: features.code_ratio,
            reasoning_level: features.reasoning_level.into(),
            domain: features.domain.into(),
            suggested_context_size: features.suggested_context_size.into(),
            analysis_time_us: features.analysis_time_us,
            has_code_blocks: features.has_code_blocks,
            question_count: features.question_count,
            imperative_count: features.imperative_count,
        }
    }
}

// ============================================================================
// P2: Semantic Cache FFI Types
// ============================================================================

/// Cache hit type for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheHitTypeFFI {
    /// Exact hash match
    Exact,
    /// Semantic similarity match
    Semantic,
}

impl From<crate::dispatcher::model_router::CacheHitType> for CacheHitTypeFFI {
    fn from(hit_type: crate::dispatcher::model_router::CacheHitType) -> Self {
        match hit_type {
            crate::dispatcher::model_router::CacheHitType::Exact => CacheHitTypeFFI::Exact,
            crate::dispatcher::model_router::CacheHitType::Semantic => CacheHitTypeFFI::Semantic,
        }
    }
}

/// Cache statistics for FFI
#[derive(Debug, Clone)]
pub struct CacheStatsFFI {
    /// Total number of entries in cache
    pub total_entries: u64,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Number of cache hits
    pub hit_count: u64,
    /// Number of cache misses
    pub miss_count: u64,
    /// Hit rate (0.0 - 1.0)
    pub hit_rate: f64,
    /// Number of exact hash hits
    pub exact_hits: u64,
    /// Number of semantic similarity hits
    pub semantic_hits: u64,
    /// Total number of evictions
    pub evictions: u64,
}

impl From<crate::dispatcher::model_router::CacheStats> for CacheStatsFFI {
    fn from(stats: crate::dispatcher::model_router::CacheStats) -> Self {
        Self {
            total_entries: stats.total_entries as u64,
            total_size_bytes: stats.total_size_bytes as u64,
            hit_count: stats.hit_count,
            miss_count: stats.miss_count,
            hit_rate: stats.hit_rate,
            exact_hits: stats.exact_hits,
            semantic_hits: stats.semantic_hits,
            evictions: stats.evictions,
        }
    }
}

impl CacheStatsFFI {
    /// Create empty cache stats (when cache is disabled)
    pub fn empty() -> Self {
        Self {
            total_entries: 0,
            total_size_bytes: 0,
            hit_count: 0,
            miss_count: 0,
            hit_rate: 0.0,
            exact_hits: 0,
            semantic_hits: 0,
            evictions: 0,
        }
    }
}

// ============================================================================
// P3: A/B Testing FFI Types
// ============================================================================

/// Experiment status for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentStatusFFI {
    Running,
    Paused,
    Completed,
    InsufficientData,
}

impl From<crate::dispatcher::model_router::ExperimentStatus> for ExperimentStatusFFI {
    fn from(status: crate::dispatcher::model_router::ExperimentStatus) -> Self {
        match status {
            crate::dispatcher::model_router::ExperimentStatus::Running => {
                ExperimentStatusFFI::Running
            }
            crate::dispatcher::model_router::ExperimentStatus::Paused => {
                ExperimentStatusFFI::Paused
            }
            crate::dispatcher::model_router::ExperimentStatus::Completed => {
                ExperimentStatusFFI::Completed
            }
            crate::dispatcher::model_router::ExperimentStatus::InsufficientData => {
                ExperimentStatusFFI::InsufficientData
            }
        }
    }
}

/// A/B experiment summary for FFI (simplified view for UI)
#[derive(Debug, Clone)]
pub struct ExperimentSummaryFFI {
    /// Unique experiment ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Current status
    pub status: ExperimentStatusFFI,
    /// Status as display string
    pub status_display: String,
    /// Whether the experiment is enabled
    pub enabled: bool,
    /// Traffic percentage (0-100)
    pub traffic_percentage: u8,
    /// Number of variants
    pub variant_count: u32,
    /// Total samples collected
    pub total_samples: u64,
    /// Duration in seconds since start
    pub duration_secs: u64,
    /// Target intent filter (if any)
    pub target_intent: Option<String>,
}

impl From<&crate::dispatcher::model_router::ExperimentReport> for ExperimentSummaryFFI {
    fn from(report: &crate::dispatcher::model_router::ExperimentReport) -> Self {
        Self {
            id: report.experiment_id.clone(),
            name: report.experiment_name.clone(),
            status: report.status.into(),
            status_display: report.status.display_name().to_string(),
            enabled: report.status != crate::dispatcher::model_router::ExperimentStatus::Paused,
            traffic_percentage: 0, // Not available in report, would need config
            variant_count: report.variant_summaries.len() as u32,
            total_samples: report.total_samples,
            duration_secs: report.duration_secs,
            target_intent: None, // Not available in report
        }
    }
}

/// Variant summary for FFI
#[derive(Debug, Clone)]
pub struct VariantSummaryFFI {
    /// Variant ID
    pub id: String,
    /// Variant name
    pub name: String,
    /// Sample count
    pub sample_count: u64,
    /// Sample percentage of total
    pub sample_percentage: f64,
    /// Mean latency (if tracked)
    pub mean_latency_ms: Option<f64>,
    /// Mean cost (if tracked)
    pub mean_cost_usd: Option<f64>,
    /// Success rate (if tracked)
    pub success_rate: Option<f64>,
}

impl From<&crate::dispatcher::model_router::VariantSummary> for VariantSummaryFFI {
    fn from(summary: &crate::dispatcher::model_router::VariantSummary) -> Self {
        let mean_latency = summary
            .metrics
            .get(&crate::dispatcher::model_router::TrackedMetric::LatencyMs)
            .map(|m| m.mean);
        let mean_cost = summary
            .metrics
            .get(&crate::dispatcher::model_router::TrackedMetric::CostUsd)
            .map(|m| m.mean);
        let success_rate = summary
            .metrics
            .get(&crate::dispatcher::model_router::TrackedMetric::SuccessRate)
            .map(|m| m.mean);

        Self {
            id: summary.variant_id.clone(),
            name: summary.variant_name.clone(),
            sample_count: summary.sample_count,
            sample_percentage: summary.sample_percentage,
            mean_latency_ms: mean_latency,
            mean_cost_usd: mean_cost,
            success_rate,
        }
    }
}

/// Significance test result for FFI
#[derive(Debug, Clone)]
pub struct SignificanceResultFFI {
    /// Metric being compared
    pub metric_name: String,
    /// Control variant ID
    pub control_id: String,
    /// Control mean value
    pub control_mean: f64,
    /// Treatment variant ID
    pub treatment_id: String,
    /// Treatment mean value
    pub treatment_mean: f64,
    /// P-value from t-test
    pub p_value: f64,
    /// Whether result is statistically significant
    pub is_significant: bool,
    /// Relative change percentage
    pub relative_change_percent: f64,
    /// Effect size (Cohen's d)
    pub effect_size: f64,
}

impl From<&crate::dispatcher::model_router::SignificanceResult> for SignificanceResultFFI {
    fn from(result: &crate::dispatcher::model_router::SignificanceResult) -> Self {
        Self {
            metric_name: result.metric.display_name(),
            control_id: result.control_id.clone(),
            control_mean: result.control_mean,
            treatment_id: result.treatment_id.clone(),
            treatment_mean: result.treatment_mean,
            p_value: result.p_value,
            is_significant: result.is_significant,
            relative_change_percent: result.relative_change * 100.0,
            effect_size: result.cohens_d,
        }
    }
}

/// Full experiment report for FFI
#[derive(Debug, Clone)]
pub struct ExperimentReportFFI {
    /// Basic experiment info
    pub summary: ExperimentSummaryFFI,
    /// Per-variant summaries
    pub variants: Vec<VariantSummaryFFI>,
    /// Significance test results
    pub significance_tests: Vec<SignificanceResultFFI>,
    /// Automated recommendation (if any)
    pub recommendation: Option<String>,
}

impl From<&crate::dispatcher::model_router::ExperimentReport> for ExperimentReportFFI {
    fn from(report: &crate::dispatcher::model_router::ExperimentReport) -> Self {
        Self {
            summary: report.into(),
            variants: report
                .variant_summaries
                .iter()
                .map(VariantSummaryFFI::from)
                .collect(),
            significance_tests: report
                .significance_tests
                .iter()
                .map(SignificanceResultFFI::from)
                .collect(),
            recommendation: report.recommendation.clone(),
        }
    }
}

/// A/B testing overview for FFI
#[derive(Debug, Clone)]
pub struct ABTestingStatusFFI {
    /// Whether A/B testing is enabled
    pub enabled: bool,
    /// Total number of experiments
    pub total_experiments: u32,
    /// Number of active experiments
    pub active_experiments: u32,
    /// List of experiment summaries
    pub experiments: Vec<ExperimentSummaryFFI>,
    /// Status emoji for quick display
    pub status_emoji: String,
    /// Human-readable status message
    pub status_message: String,
}

impl ABTestingStatusFFI {
    /// Create a disabled A/B testing status
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            total_experiments: 0,
            active_experiments: 0,
            experiments: Vec::new(),
            status_emoji: "⚫".to_string(),
            status_message: "A/B testing disabled".to_string(),
        }
    }

    /// Create from experiment reports
    pub fn from_reports(reports: &[crate::dispatcher::model_router::ExperimentReport]) -> Self {
        if reports.is_empty() {
            return Self {
                enabled: true,
                total_experiments: 0,
                active_experiments: 0,
                experiments: Vec::new(),
                status_emoji: "⚪".to_string(),
                status_message: "No experiments configured".to_string(),
            };
        }

        let active_count = reports
            .iter()
            .filter(|r| r.status == crate::dispatcher::model_router::ExperimentStatus::Running)
            .count() as u32;

        let (emoji, message) = if active_count > 0 {
            (
                "🧪".to_string(),
                format!("{} experiment(s) running", active_count),
            )
        } else {
            ("⏸️".to_string(), "No active experiments".to_string())
        };

        Self {
            enabled: true,
            total_experiments: reports.len() as u32,
            active_experiments: active_count,
            experiments: reports.iter().map(ExperimentSummaryFFI::from).collect(),
            status_emoji: emoji,
            status_message: message,
        }
    }
}

// ============================================================================
// P3: Ensemble FFI Types
// ============================================================================

/// Ensemble mode for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsembleModeFFI {
    Disabled,
    BestOfN,
    Voting,
    Consensus,
    Cascade,
}

impl From<crate::dispatcher::model_router::EnsembleMode> for EnsembleModeFFI {
    fn from(mode: crate::dispatcher::model_router::EnsembleMode) -> Self {
        match mode {
            crate::dispatcher::model_router::EnsembleMode::Disabled => EnsembleModeFFI::Disabled,
            crate::dispatcher::model_router::EnsembleMode::BestOfN { .. } => {
                EnsembleModeFFI::BestOfN
            }
            crate::dispatcher::model_router::EnsembleMode::Voting => EnsembleModeFFI::Voting,
            crate::dispatcher::model_router::EnsembleMode::Consensus { .. } => {
                EnsembleModeFFI::Consensus
            }
            crate::dispatcher::model_router::EnsembleMode::Cascade { .. } => {
                EnsembleModeFFI::Cascade
            }
        }
    }
}

/// Quality metric for FFI
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualityMetricFFI {
    Length,
    Structure,
    LengthAndStructure,
    ConfidenceMarkers,
    Relevance,
    Custom { name: String },
}

impl From<&crate::dispatcher::model_router::QualityMetric> for QualityMetricFFI {
    fn from(metric: &crate::dispatcher::model_router::QualityMetric) -> Self {
        match metric {
            crate::dispatcher::model_router::QualityMetric::Length => QualityMetricFFI::Length,
            crate::dispatcher::model_router::QualityMetric::Structure => {
                QualityMetricFFI::Structure
            }
            crate::dispatcher::model_router::QualityMetric::LengthAndStructure => {
                QualityMetricFFI::LengthAndStructure
            }
            crate::dispatcher::model_router::QualityMetric::ConfidenceMarkers => {
                QualityMetricFFI::ConfidenceMarkers
            }
            crate::dispatcher::model_router::QualityMetric::Relevance => {
                QualityMetricFFI::Relevance
            }
            crate::dispatcher::model_router::QualityMetric::Custom(name) => {
                QualityMetricFFI::Custom { name: name.clone() }
            }
        }
    }
}

/// Ensemble configuration summary for FFI
#[derive(Debug, Clone)]
pub struct EnsembleConfigSummaryFFI {
    /// Whether ensemble is enabled
    pub enabled: bool,
    /// Current ensemble mode
    pub mode: EnsembleModeFFI,
    /// Mode as display string
    pub mode_display: String,
    /// Models configured for ensemble
    pub models: Vec<String>,
    /// Default quality metric
    pub quality_metric: QualityMetricFFI,
    /// Default timeout in milliseconds
    pub timeout_ms: u64,
    /// Whether high complexity triggers ensemble
    pub high_complexity_enabled: bool,
    /// Complexity threshold for auto-triggering
    pub complexity_threshold: f64,
}

impl EnsembleConfigSummaryFFI {
    /// Create a disabled ensemble config summary
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            mode: EnsembleModeFFI::Disabled,
            mode_display: "Disabled".to_string(),
            models: Vec::new(),
            quality_metric: QualityMetricFFI::LengthAndStructure,
            timeout_ms: 30000,
            high_complexity_enabled: false,
            complexity_threshold: 0.8,
        }
    }
}

/// Ensemble execution statistics for FFI
#[derive(Debug, Clone)]
pub struct EnsembleStatsFFI {
    /// Total ensemble executions
    pub total_executions: u64,
    /// Number of successful executions
    pub successful_executions: u64,
    /// Average latency in milliseconds
    pub avg_latency_ms: f64,
    /// Average cost in USD
    pub avg_cost_usd: f64,
    /// Average confidence score
    pub avg_confidence: f64,
    /// Number of high consensus results
    pub high_consensus_count: u64,
    /// Number of low consensus results
    pub low_consensus_count: u64,
    /// Most used aggregation method
    pub most_used_method: String,
}

impl EnsembleStatsFFI {
    /// Create empty ensemble stats
    pub fn empty() -> Self {
        Self {
            total_executions: 0,
            successful_executions: 0,
            avg_latency_ms: 0.0,
            avg_cost_usd: 0.0,
            avg_confidence: 0.0,
            high_consensus_count: 0,
            low_consensus_count: 0,
            most_used_method: "none".to_string(),
        }
    }
}

/// Overall ensemble status for FFI
#[derive(Debug, Clone)]
pub struct EnsembleStatusFFI {
    /// Configuration summary
    pub config: EnsembleConfigSummaryFFI,
    /// Execution statistics
    pub stats: EnsembleStatsFFI,
    /// Status emoji for quick display
    pub status_emoji: String,
    /// Human-readable status message
    pub status_message: String,
}

impl EnsembleStatusFFI {
    /// Create a disabled ensemble status
    pub fn disabled() -> Self {
        Self {
            config: EnsembleConfigSummaryFFI::disabled(),
            stats: EnsembleStatsFFI::empty(),
            status_emoji: "⚫".to_string(),
            status_message: "Ensemble disabled".to_string(),
        }
    }
}

// ============================================================================
// Task FFI Structs
// ============================================================================

/// Cowork task for FFI (simplified)
#[derive(Debug, Clone)]
pub struct AgentTaskFFI {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub task_type: AgentTaskTypeCategory,
    pub status: AgentTaskStatusState,
    pub progress: f32,
    pub error_message: Option<String>,
}

impl From<&Task> for AgentTaskFFI {
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
            task_type: AgentTaskTypeCategory::from(&task.task_type),
            status: AgentTaskStatusState::from(&task.status),
            progress: task.progress(),
            error_message,
        }
    }
}

/// Cowork task dependency for FFI
#[derive(Debug, Clone)]
pub struct AgentTaskDependencyFFI {
    pub from_task_id: String,
    pub to_task_id: String,
}

impl From<&TaskDependency> for AgentTaskDependencyFFI {
    fn from(dep: &TaskDependency) -> Self {
        Self {
            from_task_id: dep.from.clone(),
            to_task_id: dep.to.clone(),
        }
    }
}

/// Cowork task graph for FFI
#[derive(Debug, Clone)]
pub struct AgentTaskGraphFFI {
    pub id: String,
    pub title: String,
    pub original_request: Option<String>,
    pub tasks: Vec<AgentTaskFFI>,
    pub edges: Vec<AgentTaskDependencyFFI>,
}

impl From<&TaskGraph> for AgentTaskGraphFFI {
    fn from(graph: &TaskGraph) -> Self {
        Self {
            id: graph.id.clone(),
            title: graph.metadata.title.clone(),
            original_request: graph.metadata.original_request.clone(),
            tasks: graph.tasks.iter().map(AgentTaskFFI::from).collect(),
            edges: graph
                .edges
                .iter()
                .map(AgentTaskDependencyFFI::from)
                .collect(),
        }
    }
}

/// Cowork execution summary for FFI
#[derive(Debug, Clone)]
pub struct AgentExecutionSummaryFFI {
    pub graph_id: String,
    pub total_tasks: u32,
    pub completed_tasks: u32,
    pub failed_tasks: u32,
    pub cancelled_tasks: u32,
    pub total_duration_ms: u64,
    pub errors: Vec<String>,
}

impl From<ExecutionSummary> for AgentExecutionSummaryFFI {
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
pub struct AgentProgressEventFFI {
    pub event_type: AgentProgressEventType,
    pub task_id: Option<String>,
    pub task_name: Option<String>,
    pub progress: f32,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl From<&ProgressEvent> for AgentProgressEventFFI {
    fn from(event: &ProgressEvent) -> Self {
        match event {
            ProgressEvent::TaskStarted { task_id, task_name } => Self {
                event_type: AgentProgressEventType::TaskStarted,
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
                event_type: AgentProgressEventType::TaskProgress,
                task_id: Some(task_id.clone()),
                task_name: None,
                progress: *progress,
                message: message.clone(),
                error: None,
            },
            ProgressEvent::TaskCompleted {
                task_id, task_name, ..
            } => Self {
                event_type: AgentProgressEventType::TaskCompleted,
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
                event_type: AgentProgressEventType::TaskFailed,
                task_id: Some(task_id.clone()),
                task_name: Some(task_name.clone()),
                progress: 0.0,
                message: None,
                error: Some(error.clone()),
            },
            ProgressEvent::TaskCancelled { task_id, task_name } => Self {
                event_type: AgentProgressEventType::TaskCancelled,
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
                event_type: AgentProgressEventType::GraphProgress,
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
                event_type: AgentProgressEventType::GraphCompleted,
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
pub trait AgentProgressHandler: Send + Sync {
    /// Called when a progress event occurs
    fn on_progress_event(&self, event: AgentProgressEventFFI);
}

/// Adapter to bridge FFI callback to internal ProgressSubscriber
pub struct FfiProgressSubscriber {
    handler: Arc<dyn AgentProgressHandler>,
}

impl FfiProgressSubscriber {
    /// Create a new FFI progress subscriber
    pub fn new(handler: Arc<dyn AgentProgressHandler>) -> Self {
        Self { handler }
    }
}

impl ProgressSubscriber for FfiProgressSubscriber {
    fn on_event(&self, event: ProgressEvent) {
        let ffi_event = AgentProgressEventFFI::from(&event);
        self.handler.on_progress_event(ffi_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{FileOp, TaskResult};
    use std::path::PathBuf;

    #[test]
    fn test_execution_state_conversion() {
        assert_eq!(
            AgentExecutionState::from(ExecutionState::Idle),
            AgentExecutionState::Idle
        );
        assert_eq!(
            AgentExecutionState::from(ExecutionState::Executing),
            AgentExecutionState::Executing
        );
    }

    #[test]
    fn test_task_status_conversion() {
        assert_eq!(
            AgentTaskStatusState::from(&TaskStatus::Pending),
            AgentTaskStatusState::Pending
        );
        assert_eq!(
            AgentTaskStatusState::from(&TaskStatus::running(0.5)),
            AgentTaskStatusState::Running
        );
        assert_eq!(
            AgentTaskStatusState::from(&TaskStatus::completed(TaskResult::default())),
            AgentTaskStatusState::Completed
        );
        assert_eq!(
            AgentTaskStatusState::from(&TaskStatus::failed("error")),
            AgentTaskStatusState::Failed
        );
    }

    #[test]
    fn test_config_conversion() {
        let config = AgentConfig {
            enabled: true,
            require_confirmation: false,
            max_parallelism: 8,
            dry_run: true,
            ..Default::default()
        };

        let ffi_config = AgentConfigFFI::from(config.clone());
        assert_eq!(ffi_config.enabled, true);
        assert_eq!(ffi_config.max_parallelism, 8);
        assert_eq!(ffi_config.dry_run, true);

        let converted_back = AgentConfig::from(ffi_config);
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

        let ffi_task = AgentTaskFFI::from(&task);
        assert_eq!(ffi_task.id, "task_1");
        assert_eq!(ffi_task.name, "Test Task");
        assert_eq!(ffi_task.description, Some("A test task".to_string()));
        assert_eq!(ffi_task.task_type, AgentTaskTypeCategory::FileOperation);
        assert_eq!(ffi_task.status, AgentTaskStatusState::Pending);
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

        let ffi_graph = AgentTaskGraphFFI::from(&graph);
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

        let ffi_event = AgentProgressEventFFI::from(&event);
        assert_eq!(ffi_event.event_type, AgentProgressEventType::TaskStarted);
        assert_eq!(ffi_event.task_id, Some("task_1".to_string()));
        assert_eq!(ffi_event.task_name, Some("Test Task".to_string()));
    }

    // =========================================================================
    // Model Router FFI Tests
    // =========================================================================

    #[test]
    fn test_model_capability_ffi_conversion() {
        use crate::dispatcher::model_router::Capability;

        // Test all capability conversions
        let capabilities = vec![
            (
                Capability::CodeGeneration,
                ModelCapabilityFFI::CodeGeneration,
            ),
            (Capability::CodeReview, ModelCapabilityFFI::CodeReview),
            (Capability::TextAnalysis, ModelCapabilityFFI::TextAnalysis),
            (
                Capability::ImageUnderstanding,
                ModelCapabilityFFI::ImageUnderstanding,
            ),
            (
                Capability::VideoUnderstanding,
                ModelCapabilityFFI::VideoUnderstanding,
            ),
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
        use crate::dispatcher::model_router::CostTier;

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
        use crate::dispatcher::model_router::LatencyTier;

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
        use crate::dispatcher::model_router::CostStrategy;

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
        use crate::dispatcher::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

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
        use crate::dispatcher::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

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
        use crate::dispatcher::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

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
            capability_mappings: vec![CapabilityMappingFFI {
                capability: ModelCapabilityFFI::Reasoning,
                model_id: "claude-opus".to_string(),
            }],
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

    #[test]
    fn test_model_health_status_ffi_conversion() {
        use crate::dispatcher::model_router::HealthStatus;

        let statuses = [
            (HealthStatus::Healthy, ModelHealthStatusFFI::Healthy),
            (HealthStatus::Degraded, ModelHealthStatusFFI::Degraded),
            (HealthStatus::Unhealthy, ModelHealthStatusFFI::Unhealthy),
            (HealthStatus::CircuitOpen, ModelHealthStatusFFI::CircuitOpen),
            (HealthStatus::HalfOpen, ModelHealthStatusFFI::HalfOpen),
            (HealthStatus::Unknown, ModelHealthStatusFFI::Unknown),
        ];

        for (status, expected_ffi) in statuses {
            let ffi: ModelHealthStatusFFI = status.into();
            assert_eq!(ffi, expected_ffi);

            let back: HealthStatus = ffi.into();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn test_model_health_summary_ffi_conversion() {
        use crate::dispatcher::model_router::{HealthStatus, ModelHealthSummary};

        let summary = ModelHealthSummary {
            model_id: "claude-opus".to_string(),
            status: HealthStatus::Healthy,
            status_text: "Healthy".to_string(),
            status_emoji: "✅".to_string(),
            reason: None,
            consecutive_successes: 10,
            consecutive_failures: 0,
        };

        let ffi: ModelHealthSummaryFFI = summary.clone().into();
        assert_eq!(ffi.model_id, "claude-opus");
        assert_eq!(ffi.status, ModelHealthStatusFFI::Healthy);
        assert_eq!(ffi.status_text, "Healthy");
        assert_eq!(ffi.status_emoji, "✅");
        assert!(ffi.reason.is_none());
        assert_eq!(ffi.consecutive_successes, 10);
        assert_eq!(ffi.consecutive_failures, 0);

        // Test from reference
        let ffi_ref: ModelHealthSummaryFFI = (&summary).into();
        assert_eq!(ffi_ref.model_id, "claude-opus");
    }

    #[test]
    fn test_model_health_summary_ffi_with_reason() {
        use crate::dispatcher::model_router::{HealthStatus, ModelHealthSummary};

        let summary = ModelHealthSummary {
            model_id: "gpt-4o".to_string(),
            status: HealthStatus::Degraded,
            status_text: "Degraded".to_string(),
            status_emoji: "⚠️".to_string(),
            reason: Some("High latency: p95 2500ms (threshold: 1000ms)".to_string()),
            consecutive_successes: 3,
            consecutive_failures: 2,
        };

        let ffi: ModelHealthSummaryFFI = summary.into();
        assert_eq!(ffi.status, ModelHealthStatusFFI::Degraded);
        assert!(ffi.reason.is_some());
        assert!(ffi.reason.unwrap().contains("High latency"));
    }

    #[test]
    fn test_health_statistics_ffi_conversion() {
        use crate::dispatcher::model_router::HealthStatistics;

        let stats = HealthStatistics {
            total: 5,
            healthy: 3,
            degraded: 1,
            unhealthy: 0,
            circuit_open: 0,
            half_open: 0,
            unknown: 1,
        };

        let ffi: HealthStatisticsFFI = stats.clone().into();
        assert_eq!(ffi.total, 5);
        assert_eq!(ffi.healthy, 3);
        assert_eq!(ffi.degraded, 1);
        assert_eq!(ffi.unhealthy, 0);
        assert_eq!(ffi.circuit_open, 0);
        assert_eq!(ffi.half_open, 0);
        assert_eq!(ffi.unknown, 1);
        assert!((ffi.healthy_percent - 60.0).abs() < 0.01); // 3/5 = 60%

        // Test from reference
        let ffi_ref: HealthStatisticsFFI = (&stats).into();
        assert_eq!(ffi_ref.total, 5);
    }

    // =========================================================================
    // Budget FFI Tests
    // =========================================================================

    #[test]
    fn test_budget_scope_ffi_conversion() {
        use crate::dispatcher::model_router::BudgetScope;

        // Global scope
        let global = BudgetScope::Global;
        let ffi: BudgetScopeFFI = (&global).into();
        assert_eq!(ffi, BudgetScopeFFI::Global);
        let back: BudgetScope = ffi.into();
        assert_eq!(back, global);

        // Project scope
        let project = BudgetScope::Project("test-project".to_string());
        let ffi: BudgetScopeFFI = (&project).into();
        assert_eq!(
            ffi,
            BudgetScopeFFI::Project {
                id: "test-project".to_string()
            }
        );
        let back: BudgetScope = ffi.into();
        assert_eq!(back, project);

        // Session scope
        let session = BudgetScope::Session("session-123".to_string());
        let ffi: BudgetScopeFFI = (&session).into();
        assert_eq!(
            ffi,
            BudgetScopeFFI::Session {
                id: "session-123".to_string()
            }
        );

        // Model scope
        let model = BudgetScope::Model("claude-opus".to_string());
        let ffi: BudgetScopeFFI = (&model).into();
        assert_eq!(
            ffi,
            BudgetScopeFFI::Model {
                id: "claude-opus".to_string()
            }
        );
    }

    #[test]
    fn test_budget_period_ffi_conversion() {
        use crate::dispatcher::model_router::BudgetPeriod;

        let periods = [
            (BudgetPeriod::Lifetime, BudgetPeriodFFI::Lifetime),
            (BudgetPeriod::daily(), BudgetPeriodFFI::Daily),
            (BudgetPeriod::weekly(), BudgetPeriodFFI::Weekly),
            (BudgetPeriod::monthly(), BudgetPeriodFFI::Monthly),
        ];

        for (period, expected_ffi) in periods {
            let ffi: BudgetPeriodFFI = (&period).into();
            assert_eq!(ffi, expected_ffi);
        }
    }

    #[test]
    fn test_budget_enforcement_ffi_conversion() {
        use crate::dispatcher::model_router::BudgetEnforcement;

        let enforcements = [
            (BudgetEnforcement::WarnOnly, BudgetEnforcementFFI::WarnOnly),
            (
                BudgetEnforcement::SoftBlock,
                BudgetEnforcementFFI::SoftBlock,
            ),
            (
                BudgetEnforcement::HardBlock,
                BudgetEnforcementFFI::HardBlock,
            ),
        ];

        for (enforcement, expected_ffi) in enforcements {
            let ffi: BudgetEnforcementFFI = enforcement.into();
            assert_eq!(ffi, expected_ffi);

            let back: BudgetEnforcement = ffi.into();
            assert_eq!(back, enforcement);
        }
    }

    #[test]
    fn test_budget_limit_status_ffi_creation() {
        use crate::dispatcher::model_router::{
            BudgetLimit, BudgetPeriod, BudgetScope, BudgetState,
        };

        let limit = BudgetLimit::new("daily-global", 10.0)
            .with_scope(BudgetScope::Global)
            .with_period(BudgetPeriod::daily());

        let state = BudgetState::new(&limit);

        let ffi = BudgetLimitStatusFFI::from_limit_and_state(&limit, &state);

        assert_eq!(ffi.limit_id, "daily-global");
        assert_eq!(ffi.scope, BudgetScopeFFI::Global);
        assert_eq!(ffi.scope_display, "global");
        assert_eq!(ffi.period, BudgetPeriodFFI::Daily);
        assert_eq!(ffi.period_display, "daily");
        assert!((ffi.limit_usd - 10.0).abs() < 0.001);
        assert!((ffi.spent_usd - 0.0).abs() < 0.001);
        assert!((ffi.remaining_usd - 10.0).abs() < 0.001);
        assert!((ffi.used_percent - 0.0).abs() < 0.001);
        assert!(!ffi.is_exceeded);
        assert!(!ffi.is_warning);
    }

    #[test]
    fn test_budget_status_ffi_disabled() {
        let status = BudgetStatusFFI::disabled();

        assert!(!status.enabled);
        assert_eq!(status.total_limits, 0);
        assert_eq!(status.exceeded_count, 0);
        assert_eq!(status.warning_count, 0);
        assert!(status.limits.is_empty());
        assert_eq!(status.status_emoji, "⚫");
        assert!(status.status_message.contains("disabled"));
    }

    #[test]
    fn test_budget_status_ffi_from_limits() {
        use crate::dispatcher::model_router::{
            BudgetLimit, BudgetPeriod, BudgetScope, BudgetState,
        };
        use std::collections::HashMap;

        // Create some test limits
        let limits = vec![
            BudgetLimit::new("daily", 10.0)
                .with_scope(BudgetScope::Global)
                .with_period(BudgetPeriod::daily()),
            BudgetLimit::new("monthly", 100.0)
                .with_scope(BudgetScope::Global)
                .with_period(BudgetPeriod::monthly()),
        ];

        // Create states
        let mut states = HashMap::new();
        for limit in &limits {
            states.insert(limit.id.clone(), BudgetState::new(limit));
        }

        let status = BudgetStatusFFI::from_limits_and_states(&limits, &states);

        assert!(status.enabled);
        assert_eq!(status.total_limits, 2);
        assert_eq!(status.exceeded_count, 0);
        assert_eq!(status.warning_count, 0);
        assert_eq!(status.limits.len(), 2);
        assert_eq!(status.status_emoji, "🟢");
        assert!(status.status_message.contains("healthy"));
    }
}
