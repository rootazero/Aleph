//! Execution plan types for the unified planner
//!
//! This module defines the core types for the unified planner architecture:
//! - `ExecutionPlan`: The output of the planner, representing different execution strategies
//! - `PlannedTask`: A task within a task graph
//! - `PlannerError`: Error types for planner operations

use serde::{Deserialize, Serialize};

use crate::cowork::types::{Task, TaskType};

/// Execution plan - output of the unified planner
///
/// Represents the three possible execution strategies:
/// - Conversational: Pure conversation, no tools needed
/// - SingleAction: A single tool call or simple task
/// - TaskGraph: Complex multi-step task with dependencies
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionPlan {
    /// Pure conversation, no tools needed
    Conversational {
        /// Optional enhanced prompt for the conversation
        enhanced_prompt: Option<String>,
    },

    /// Single action (tool call or simple task)
    SingleAction {
        /// Name of the tool to call
        tool_name: String,
        /// Parameters for the tool
        parameters: serde_json::Value,
        /// Whether user confirmation is required before execution
        requires_confirmation: bool,
    },

    /// Complex task graph (multi-step)
    TaskGraph {
        /// List of planned tasks
        tasks: Vec<PlannedTask>,
        /// Dependencies as (dependent_task_id, dependency_task_id) pairs
        dependencies: Vec<(usize, usize)>,
        /// Whether user confirmation is required before execution
        requires_confirmation: bool,
    },
}

impl ExecutionPlan {
    /// Check if this plan requires user confirmation before execution
    pub fn requires_confirmation(&self) -> bool {
        match self {
            ExecutionPlan::Conversational { .. } => false,
            ExecutionPlan::SingleAction {
                requires_confirmation,
                ..
            } => *requires_confirmation,
            ExecutionPlan::TaskGraph {
                requires_confirmation,
                ..
            } => *requires_confirmation,
        }
    }

    /// Get the type name of this plan
    pub fn plan_type(&self) -> &'static str {
        match self {
            ExecutionPlan::Conversational { .. } => "conversational",
            ExecutionPlan::SingleAction { .. } => "single_action",
            ExecutionPlan::TaskGraph { .. } => "task_graph",
        }
    }

    /// Create a conversational plan with no enhanced prompt
    pub fn conversational() -> Self {
        ExecutionPlan::Conversational {
            enhanced_prompt: None,
        }
    }

    /// Create a conversational plan with an enhanced prompt
    pub fn conversational_with_prompt(prompt: String) -> Self {
        ExecutionPlan::Conversational {
            enhanced_prompt: Some(prompt),
        }
    }

    /// Create a single action plan without confirmation requirement
    pub fn single_action(tool_name: String, parameters: serde_json::Value) -> Self {
        ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation: false,
        }
    }

    /// Create a single action plan that requires user confirmation
    pub fn single_action_with_confirmation(
        tool_name: String,
        parameters: serde_json::Value,
    ) -> Self {
        ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation: true,
        }
    }

    /// Create a task graph plan from tasks and dependencies
    pub fn task_graph(tasks: Vec<PlannedTask>, dependencies: Vec<(usize, usize)>) -> Self {
        ExecutionPlan::TaskGraph {
            tasks,
            dependencies,
            requires_confirmation: false,
        }
    }

    /// Create a task graph plan that requires user confirmation
    pub fn task_graph_with_confirmation(
        tasks: Vec<PlannedTask>,
        dependencies: Vec<(usize, usize)>,
    ) -> Self {
        ExecutionPlan::TaskGraph {
            tasks,
            dependencies,
            requires_confirmation: true,
        }
    }

    /// Get the number of tasks in this plan
    pub fn task_count(&self) -> usize {
        match self {
            ExecutionPlan::Conversational { .. } => 0,
            ExecutionPlan::SingleAction { .. } => 1,
            ExecutionPlan::TaskGraph { tasks, .. } => tasks.len(),
        }
    }

    /// Check if this is a conversational plan
    pub fn is_conversational(&self) -> bool {
        matches!(self, ExecutionPlan::Conversational { .. })
    }

    /// Check if this is a single action plan
    pub fn is_single_action(&self) -> bool {
        matches!(self, ExecutionPlan::SingleAction { .. })
    }

    /// Check if this is a task graph plan
    pub fn is_task_graph(&self) -> bool {
        matches!(self, ExecutionPlan::TaskGraph { .. })
    }
}

/// A planned task within a TaskGraph
///
/// Represents a single step in a multi-step execution plan.
/// Can be converted to a `Task` for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTask {
    /// Unique identifier for this task within the graph
    pub id: usize,
    /// Human-readable description of what this task does
    pub description: String,
    /// Type of task (determines which executor handles it)
    pub task_type: TaskType,
    /// Optional hint for which tool to use
    pub tool_hint: Option<String>,
    /// Task-specific parameters
    pub parameters: serde_json::Value,
}

impl PlannedTask {
    /// Create a new planned task with minimal fields
    pub fn new(id: usize, description: impl Into<String>, task_type: TaskType) -> Self {
        Self {
            id,
            description: description.into(),
            task_type,
            tool_hint: None,
            parameters: serde_json::Value::Null,
        }
    }

    /// Builder: set a tool hint
    pub fn with_tool_hint(mut self, tool: impl Into<String>) -> Self {
        self.tool_hint = Some(tool.into());
        self
    }

    /// Builder: set parameters
    pub fn with_parameters(mut self, params: serde_json::Value) -> Self {
        self.parameters = params;
        self
    }

    /// Convert this planned task to an executable Task
    pub fn to_task(&self) -> Task {
        Task::new(
            format!("planned_task_{}", self.id),
            &self.description,
            self.task_type.clone(),
        )
        .with_parameters(self.parameters.clone())
        .with_description(&self.description)
    }
}

/// Planner error types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PlannerError {
    /// Error from LLM call
    LlmError(String),
    /// Error parsing LLM response
    ParseError(String),
    /// Plan validation failed
    ValidationError(String),
    /// Planning timed out
    Timeout,
}

impl std::fmt::Display for PlannerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlannerError::LlmError(msg) => write!(f, "LLM error: {}", msg),
            PlannerError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            PlannerError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            PlannerError::Timeout => write!(f, "Planner timed out"),
        }
    }
}

impl std::error::Error for PlannerError {}

impl PlannerError {
    /// Create an LLM error
    pub fn llm_error(msg: impl Into<String>) -> Self {
        PlannerError::LlmError(msg.into())
    }

    /// Create a parse error
    pub fn parse_error(msg: impl Into<String>) -> Self {
        PlannerError::ParseError(msg.into())
    }

    /// Create a validation error
    pub fn validation_error(msg: impl Into<String>) -> Self {
        PlannerError::ValidationError(msg.into())
    }

    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        match self {
            PlannerError::LlmError(_) => true,         // Can retry
            PlannerError::ParseError(_) => true,       // Can retry with different prompt
            PlannerError::ValidationError(_) => false, // Plan is invalid
            PlannerError::Timeout => true,             // Can retry
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{AiTask, FileOp};
    use std::path::PathBuf;

    #[test]
    fn test_execution_plan_conversational() {
        let plan = ExecutionPlan::conversational();
        assert!(plan.is_conversational());
        assert!(!plan.requires_confirmation());
        assert_eq!(plan.plan_type(), "conversational");
        assert_eq!(plan.task_count(), 0);
    }

    #[test]
    fn test_execution_plan_conversational_with_prompt() {
        let plan = ExecutionPlan::conversational_with_prompt("Enhanced prompt".to_string());

        if let ExecutionPlan::Conversational { enhanced_prompt } = plan {
            assert_eq!(enhanced_prompt, Some("Enhanced prompt".to_string()));
        } else {
            panic!("Expected Conversational variant");
        }
    }

    #[test]
    fn test_execution_plan_single_action() {
        let params = serde_json::json!({"path": "/tmp/test.txt"});
        let plan = ExecutionPlan::single_action("read_file".to_string(), params.clone());

        assert!(plan.is_single_action());
        assert!(!plan.requires_confirmation());
        assert_eq!(plan.plan_type(), "single_action");
        assert_eq!(plan.task_count(), 1);

        if let ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation,
        } = plan
        {
            assert_eq!(tool_name, "read_file");
            assert_eq!(parameters, params);
            assert!(!requires_confirmation);
        } else {
            panic!("Expected SingleAction variant");
        }
    }

    #[test]
    fn test_execution_plan_single_action_with_confirmation() {
        let params = serde_json::json!({"path": "/important/file.txt"});
        let plan =
            ExecutionPlan::single_action_with_confirmation("delete_file".to_string(), params);

        assert!(plan.requires_confirmation());
    }

    #[test]
    fn test_execution_plan_task_graph() {
        let tasks = vec![
            PlannedTask::new(
                0,
                "Read config",
                TaskType::FileOperation(FileOp::Read {
                    path: PathBuf::from("/etc/config"),
                }),
            ),
            PlannedTask::new(
                1,
                "Process data",
                TaskType::AiInference(AiTask {
                    prompt: "Process the config".to_string(),
                    requires_privacy: false,
                    has_images: false,
                    output_format: None,
                }),
            ),
        ];
        let dependencies = vec![(1, 0)]; // Task 1 depends on task 0

        let plan = ExecutionPlan::task_graph(tasks.clone(), dependencies.clone());

        assert!(plan.is_task_graph());
        assert!(!plan.requires_confirmation());
        assert_eq!(plan.plan_type(), "task_graph");
        assert_eq!(plan.task_count(), 2);

        if let ExecutionPlan::TaskGraph {
            tasks: plan_tasks,
            dependencies: plan_deps,
            ..
        } = plan
        {
            assert_eq!(plan_tasks.len(), 2);
            assert_eq!(plan_deps, vec![(1, 0)]);
        } else {
            panic!("Expected TaskGraph variant");
        }
    }

    #[test]
    fn test_execution_plan_task_graph_with_confirmation() {
        let tasks = vec![PlannedTask::new(
            0,
            "Delete all files",
            TaskType::FileOperation(FileOp::Delete {
                path: PathBuf::from("/tmp/test"),
            }),
        )];

        let plan = ExecutionPlan::task_graph_with_confirmation(tasks, vec![]);
        assert!(plan.requires_confirmation());
    }

    #[test]
    fn test_planned_task_creation() {
        let task = PlannedTask::new(
            0,
            "Read file",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/tmp/test.txt"),
            }),
        );

        assert_eq!(task.id, 0);
        assert_eq!(task.description, "Read file");
        assert!(task.tool_hint.is_none());
        assert_eq!(task.parameters, serde_json::Value::Null);
    }

    #[test]
    fn test_planned_task_builder() {
        let params = serde_json::json!({"encoding": "utf-8"});
        let task = PlannedTask::new(
            1,
            "Read config",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/etc/config"),
            }),
        )
        .with_tool_hint("read_file")
        .with_parameters(params.clone());

        assert_eq!(task.tool_hint, Some("read_file".to_string()));
        assert_eq!(task.parameters, params);
    }

    #[test]
    fn test_planned_task_to_task() {
        let params = serde_json::json!({"key": "value"});
        let planned = PlannedTask::new(
            42,
            "Test task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
        .with_parameters(params.clone());

        let task = planned.to_task();

        assert_eq!(task.id, "planned_task_42");
        assert_eq!(task.name, "Test task");
        assert_eq!(task.description, Some("Test task".to_string()));
        assert_eq!(task.parameters, params);
    }

    #[test]
    fn test_planner_error_display() {
        assert_eq!(
            PlannerError::LlmError("API error".to_string()).to_string(),
            "LLM error: API error"
        );
        assert_eq!(
            PlannerError::ParseError("Invalid JSON".to_string()).to_string(),
            "Parse error: Invalid JSON"
        );
        assert_eq!(
            PlannerError::ValidationError("Invalid plan".to_string()).to_string(),
            "Validation error: Invalid plan"
        );
        assert_eq!(PlannerError::Timeout.to_string(), "Planner timed out");
    }

    #[test]
    fn test_planner_error_constructors() {
        let llm_err = PlannerError::llm_error("Connection failed");
        assert_eq!(
            llm_err,
            PlannerError::LlmError("Connection failed".to_string())
        );

        let parse_err = PlannerError::parse_error("Bad format");
        assert_eq!(
            parse_err,
            PlannerError::ParseError("Bad format".to_string())
        );

        let validation_err = PlannerError::validation_error("Cycle detected");
        assert_eq!(
            validation_err,
            PlannerError::ValidationError("Cycle detected".to_string())
        );
    }

    #[test]
    fn test_planner_error_is_recoverable() {
        assert!(PlannerError::LlmError("Error".to_string()).is_recoverable());
        assert!(PlannerError::ParseError("Error".to_string()).is_recoverable());
        assert!(!PlannerError::ValidationError("Error".to_string()).is_recoverable());
        assert!(PlannerError::Timeout.is_recoverable());
    }

    #[test]
    fn test_execution_plan_serialization() {
        let plan = ExecutionPlan::single_action(
            "test_tool".to_string(),
            serde_json::json!({"arg": "value"}),
        );

        let json = serde_json::to_string(&plan).expect("Serialization should succeed");
        let deserialized: ExecutionPlan =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        if let ExecutionPlan::SingleAction { tool_name, .. } = deserialized {
            assert_eq!(tool_name, "test_tool");
        } else {
            panic!("Expected SingleAction after deserialization");
        }
    }

    #[test]
    fn test_planned_task_serialization() {
        let task = PlannedTask::new(
            0,
            "Test",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
        .with_tool_hint("list_dir")
        .with_parameters(serde_json::json!({"recursive": true}));

        let json = serde_json::to_string(&task).expect("Serialization should succeed");
        let deserialized: PlannedTask =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized.id, task.id);
        assert_eq!(deserialized.description, task.description);
        assert_eq!(deserialized.tool_hint, task.tool_hint);
    }

    #[test]
    fn test_planner_error_serialization() {
        let error = PlannerError::LlmError("Test error".to_string());

        let json = serde_json::to_string(&error).expect("Serialization should succeed");
        let deserialized: PlannerError =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized, error);
    }

    #[test]
    fn test_execution_plan_json_format() {
        // Test that the JSON format uses snake_case type field
        let plan = ExecutionPlan::conversational();
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"type\":\"conversational\""));

        let plan = ExecutionPlan::single_action("test".to_string(), serde_json::json!({}));
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"type\":\"single_action\""));

        let plan = ExecutionPlan::task_graph(vec![], vec![]);
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"type\":\"task_graph\""));
    }
}
