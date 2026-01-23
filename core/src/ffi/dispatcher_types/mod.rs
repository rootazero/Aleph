//! Dispatcher FFI Types
//!
//! This module provides FFI-safe wrapper types for the Dispatcher layer:
//! - Task orchestration (scheduler, executor, planner)
//! - Model routing (profiles, rules, health monitoring)
//! - Budget management
//! - A/B testing and ensemble
//!
//! These types are designed to work with UniFFI for Swift/Kotlin interop.

mod agent;
mod budget;
mod config;
mod features;
mod health;
mod model;

// Re-export all public types

// Agent enums and task types
pub use agent::{
    AgentExecutionSummaryFFI, AgentExecutionState, AgentProgressEventFFI, AgentProgressEventType,
    AgentProgressHandler, AgentTaskDependencyFFI, AgentTaskFFI, AgentTaskGraphFFI,
    AgentTaskStatusState, AgentTaskTypeCategory, FfiProgressSubscriber,
};

// Config types
pub use config::{CodeExecConfigFFI, FileOpsConfigFFI};

// Model router types
pub use model::{
    CapabilityMappingFFI, ModelCapabilityFFI, ModelCostStrategyFFI, ModelCostTierFFI,
    ModelLatencyTierFFI, ModelProfileFFI, ModelRoutingRulesFFI, StageResultFFI,
    TaskTypeMappingFFI,
};

// Health monitoring types
pub use health::{HealthStatisticsFFI, ModelHealthStatusFFI, ModelHealthSummaryFFI};

// Budget types
pub use budget::{
    BudgetEnforcementFFI, BudgetLimitStatusFFI, BudgetPeriodFFI, BudgetScopeFFI, BudgetStatusFFI,
};

// P2/P3 feature types
pub use features::{
    // P2: Prompt Analysis
    ContextSizeFFI,
    DomainFFI,
    LanguageFFI,
    PromptFeaturesFFI,
    ReasoningLevelFFI,
    // P2: Semantic Cache
    CacheHitTypeFFI,
    CacheStatsFFI,
    // P3: A/B Testing
    ABTestingStatusFFI,
    ExperimentReportFFI,
    ExperimentStatusFFI,
    ExperimentSummaryFFI,
    SignificanceResultFFI,
    VariantSummaryFFI,
    // P3: Ensemble
    EnsembleConfigSummaryFFI,
    EnsembleModeFFI,
    EnsembleStatsFFI,
    EnsembleStatusFFI,
    QualityMetricFFI,
};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{FileOp, Task, TaskGraph, TaskType};
    use crate::dispatcher::monitor::ProgressEvent;
    use std::path::PathBuf;

    #[test]
    fn test_execution_state_conversion() {
        use crate::dispatcher::ExecutionState;

        let states = vec![
            (ExecutionState::Idle, AgentExecutionState::Idle),
            (ExecutionState::Planning, AgentExecutionState::Planning),
            (
                ExecutionState::AwaitingConfirmation,
                AgentExecutionState::AwaitingConfirmation,
            ),
            (ExecutionState::Executing, AgentExecutionState::Executing),
            (ExecutionState::Paused, AgentExecutionState::Paused),
            (ExecutionState::Cancelled, AgentExecutionState::Cancelled),
            (ExecutionState::Completed, AgentExecutionState::Completed),
        ];

        for (state, expected) in states {
            assert_eq!(
                AgentExecutionState::from(state),
                expected,
                "Failed for {:?}",
                state
            );
        }
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
        // Note: add_dependency creates from->to edges, which map to edges in the FFI
        assert!(ffi_graph.edges.iter().any(|e| e.from_task_id == "task_1" && e.to_task_id == "task_2"));
    }

    #[test]
    fn test_progress_event_conversion() {
        let event = ProgressEvent::task_started("task_1", "Test Task");

        let ffi_event = AgentProgressEventFFI::from(&event);
        assert_eq!(ffi_event.event_type, AgentProgressEventType::TaskStarted);
        assert_eq!(ffi_event.task_id, Some("task_1".to_string()));
        assert_eq!(ffi_event.task_name, Some("Test Task".to_string()));
    }
}
