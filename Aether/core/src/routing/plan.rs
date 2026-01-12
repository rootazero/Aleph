//! Task Plan Data Structures for L3 Agent Planning
//!
//! Core data structures for multi-step task planning and execution:
//!
//! - `TaskPlan`: Execution plan for multi-step tasks
//! - `PlanStep`: Single step in an execution plan
//! - `ToolSafetyLevel`: Safety classification for tools
//! - `StepResult`: Result from executing a single step
//! - `PlanExecutionContext`: Execution context for plan executor

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// Tool Safety Level
// =============================================================================

/// Tool safety classification for confirmation decisions and rollback behavior
///
/// Each tool has a safety level that determines:
/// - Whether user confirmation is required before execution
/// - Whether the operation can be rolled back on failure
/// - UI warnings to display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolSafetyLevel {
    /// Read-only operations (search, query, read file)
    ///
    /// These operations don't modify any state and are always safe to execute.
    /// Examples: web search, file read, database query
    #[default]
    ReadOnly,

    /// Operations that can be undone (copy file, create file)
    ///
    /// These operations modify state but can be reversed.
    /// Examples: create file (delete to undo), copy (delete copy to undo)
    Reversible,

    /// Cannot be undone but low risk (send notification)
    ///
    /// These operations cannot be reversed but have limited impact.
    /// Examples: send notification, log entry, API call with no side effects
    IrreversibleLowRisk,

    /// Cannot be undone and high risk (delete, execute command)
    ///
    /// These operations cannot be reversed and may have significant impact.
    /// Examples: delete file, send email, execute shell command
    IrreversibleHighRisk,
}

impl ToolSafetyLevel {
    /// Check if this level requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            ToolSafetyLevel::IrreversibleLowRisk | ToolSafetyLevel::IrreversibleHighRisk
        )
    }

    /// Check if operations at this level can be rolled back
    pub fn is_reversible(&self) -> bool {
        matches!(
            self,
            ToolSafetyLevel::ReadOnly | ToolSafetyLevel::Reversible
        )
    }

    /// Get a human-readable label for UI display
    pub fn label(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "Read Only",
            ToolSafetyLevel::Reversible => "Reversible",
            ToolSafetyLevel::IrreversibleLowRisk => "Low Risk",
            ToolSafetyLevel::IrreversibleHighRisk => "High Risk",
        }
    }

    /// Get an icon hint for UI (SF Symbol name)
    pub fn icon_hint(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "eye",
            ToolSafetyLevel::Reversible => "arrow.uturn.backward",
            ToolSafetyLevel::IrreversibleLowRisk => "exclamationmark.circle",
            ToolSafetyLevel::IrreversibleHighRisk => "exclamationmark.triangle.fill",
        }
    }

    /// Get color hint for UI
    pub fn color_hint(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "green",
            ToolSafetyLevel::Reversible => "blue",
            ToolSafetyLevel::IrreversibleLowRisk => "orange",
            ToolSafetyLevel::IrreversibleHighRisk => "red",
        }
    }
}

// =============================================================================
// Plan Step
// =============================================================================

/// Single step in an execution plan
///
/// Each step represents a tool invocation with parameters.
/// Steps are executed sequentially, and results can be passed
/// between steps using the `$prev` reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step index (1-based for display)
    pub index: u32,

    /// Tool name to execute
    ///
    /// Must match a registered tool in the ToolRegistry.
    pub tool_name: String,

    /// Tool parameters (may contain $prev reference)
    ///
    /// Parameters can reference the previous step's output using "$prev".
    /// Example: `{"content": "$prev", "format": "markdown"}`
    pub parameters: Value,

    /// Human-readable step description
    ///
    /// Displayed in the plan confirmation UI.
    pub description: String,

    /// Safety level of this step
    ///
    /// Determined by the tool being invoked.
    #[serde(default)]
    pub safety_level: ToolSafetyLevel,

    /// Maximum execution time for this step (milliseconds)
    ///
    /// Step will be terminated if it exceeds this timeout.
    #[serde(default = "default_step_timeout")]
    pub timeout_ms: u64,
}

fn default_step_timeout() -> u64 {
    30_000 // 30 seconds
}

impl PlanStep {
    /// Create a new plan step
    pub fn new(
        index: u32,
        tool_name: impl Into<String>,
        parameters: Value,
        description: impl Into<String>,
    ) -> Self {
        Self {
            index,
            tool_name: tool_name.into(),
            parameters,
            description: description.into(),
            safety_level: ToolSafetyLevel::default(),
            timeout_ms: default_step_timeout(),
        }
    }

    /// Builder: set safety level
    pub fn with_safety_level(mut self, level: ToolSafetyLevel) -> Self {
        self.safety_level = level;
        self
    }

    /// Builder: set timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Check if this step contains a $prev reference
    pub fn has_prev_reference(&self) -> bool {
        let params_str = serde_json::to_string(&self.parameters).unwrap_or_default();
        params_str.contains("$prev")
    }
}

// =============================================================================
// Task Plan
// =============================================================================

/// Execution plan for multi-step tasks
///
/// A TaskPlan contains an ordered list of steps to execute.
/// Steps are executed sequentially, with results passed between steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    /// Unique plan identifier
    pub id: Uuid,

    /// Natural language description of the plan
    ///
    /// Summarizes what the plan will accomplish.
    pub description: String,

    /// Ordered list of execution steps
    pub steps: Vec<PlanStep>,

    /// Overall confidence score (0.0-1.0)
    ///
    /// Confidence from the LLM about this plan being correct.
    pub confidence: f32,

    /// Whether plan requires user confirmation
    ///
    /// Set to true if confidence is below threshold or has irreversible steps.
    pub requires_confirmation: bool,

    /// Estimated total duration hint (optional)
    ///
    /// Human-readable estimate like "~10 seconds" or "1-2 minutes".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_duration_hint: Option<String>,

    /// Whether plan contains irreversible operations
    ///
    /// True if any step has `IrreversibleLowRisk` or `IrreversibleHighRisk`.
    pub has_irreversible_steps: bool,
}

impl TaskPlan {
    /// Create a new task plan
    pub fn new(description: impl Into<String>, steps: Vec<PlanStep>) -> Self {
        let has_irreversible = steps.iter().any(|s| !s.safety_level.is_reversible());

        Self {
            id: Uuid::new_v4(),
            description: description.into(),
            steps,
            confidence: 0.0,
            requires_confirmation: true, // Default to requiring confirmation
            estimated_duration_hint: None,
            has_irreversible_steps: has_irreversible,
        }
    }

    /// Builder: set confidence
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Builder: set requires_confirmation
    pub fn with_requires_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Builder: set estimated duration hint
    pub fn with_duration_hint(mut self, hint: impl Into<String>) -> Self {
        self.estimated_duration_hint = Some(hint.into());
        self
    }

    /// Get the number of steps in this plan
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Check if this plan is empty
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get a step by index (0-based internal index)
    pub fn get_step(&self, index: usize) -> Option<&PlanStep> {
        self.steps.get(index)
    }

    /// Update has_irreversible_steps based on current steps
    pub fn update_irreversible_flag(&mut self) {
        self.has_irreversible_steps = self.steps.iter().any(|s| !s.safety_level.is_reversible());
    }

    /// Convert to PlanInfo for UniFFI
    pub fn to_info(&self) -> PlanInfo {
        PlanInfo {
            plan_id: self.id.to_string(),
            description: self.description.clone(),
            steps: self
                .steps
                .iter()
                .map(|s| PlanStepInfo {
                    index: s.index,
                    tool_name: s.tool_name.clone(),
                    description: s.description.clone(),
                    safety_level: s.safety_level.label().to_string(),
                })
                .collect(),
            has_irreversible_steps: self.has_irreversible_steps,
        }
    }
}

// =============================================================================
// Step Result
// =============================================================================

/// Result from executing a single step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step index that produced this result
    pub step_index: u32,

    /// Output data (available to subsequent steps as $prev)
    pub output: Value,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Whether step succeeded
    pub success: bool,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Rollback data (for reversible operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_data: Option<Value>,
}

impl StepResult {
    /// Create a successful step result
    pub fn success(step_index: u32, output: Value, duration_ms: u64) -> Self {
        Self {
            step_index,
            output,
            duration_ms,
            success: true,
            error: None,
            rollback_data: None,
        }
    }

    /// Create a failed step result
    pub fn failure(step_index: u32, error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            step_index,
            output: Value::Null,
            duration_ms,
            success: false,
            error: Some(error.into()),
            rollback_data: None,
        }
    }

    /// Builder: set rollback data
    pub fn with_rollback_data(mut self, data: Value) -> Self {
        self.rollback_data = Some(data);
        self
    }
}

// =============================================================================
// Plan Execution Context
// =============================================================================

/// Execution context for plan executor
///
/// Tracks the state of a running plan execution, including
/// completed steps, current position, and rollback data.
#[derive(Debug)]
pub struct PlanExecutionContext {
    /// Plan being executed
    pub plan: TaskPlan,

    /// Results from completed steps
    pub step_results: Vec<StepResult>,

    /// Current step index (0-based)
    pub current_step: usize,

    /// Rollback data for reversible steps
    ///
    /// Stored as (step_index, rollback_data) pairs.
    pub rollback_data: Vec<(u32, Value)>,

    /// Whether execution has been cancelled
    pub cancelled: bool,
}

impl PlanExecutionContext {
    /// Create a new execution context for a plan
    pub fn new(plan: TaskPlan) -> Self {
        Self {
            plan,
            step_results: Vec::new(),
            current_step: 0,
            rollback_data: Vec::new(),
            cancelled: false,
        }
    }

    /// Get the previous step's output (for $prev resolution)
    pub fn prev_output(&self) -> Option<&Value> {
        self.step_results.last().map(|r| &r.output)
    }

    /// Add a step result
    pub fn add_result(&mut self, result: StepResult) {
        // Store rollback data if present
        if let Some(ref data) = result.rollback_data {
            self.rollback_data.push((result.step_index, data.clone()));
        }
        self.step_results.push(result);
        self.current_step += 1;
    }

    /// Check if all steps have been executed
    pub fn is_complete(&self) -> bool {
        self.current_step >= self.plan.steps.len()
    }

    /// Get the final output (last step's output)
    pub fn final_output(&self) -> Value {
        self.step_results
            .last()
            .map(|r| r.output.clone())
            .unwrap_or(Value::Null)
    }

    /// Cancel execution
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Check if any step failed
    pub fn has_failure(&self) -> bool {
        self.step_results.iter().any(|r| !r.success)
    }
}

// =============================================================================
// Plan Execution Result
// =============================================================================

/// Final result of plan execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanExecutionResult {
    /// Plan ID that was executed
    pub plan_id: Uuid,

    /// Final output from the last step
    pub final_output: Value,

    /// Results from all executed steps
    pub step_results: Vec<StepResult>,

    /// Total execution duration in milliseconds
    pub total_duration_ms: u64,

    /// Whether execution completed successfully
    pub success: bool,

    /// Error message if execution failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PlanExecutionResult {
    /// Create a successful execution result
    pub fn success(plan_id: Uuid, final_output: Value, step_results: Vec<StepResult>) -> Self {
        let total_duration_ms = step_results.iter().map(|r| r.duration_ms).sum();
        Self {
            plan_id,
            final_output,
            step_results,
            total_duration_ms,
            success: true,
            error: None,
        }
    }

    /// Create a failed execution result
    pub fn failure(
        plan_id: Uuid,
        error: impl Into<String>,
        step_results: Vec<StepResult>,
    ) -> Self {
        let total_duration_ms = step_results.iter().map(|r| r.duration_ms).sum();
        Self {
            plan_id,
            final_output: Value::Null,
            step_results,
            total_duration_ms,
            success: false,
            error: Some(error.into()),
        }
    }
}

// =============================================================================
// UniFFI Types for Plan Events
// =============================================================================

/// Plan information for UI display (UniFFI compatible)
#[derive(Debug, Clone)]
pub struct PlanInfo {
    /// Plan ID
    pub plan_id: String,
    /// Plan description
    pub description: String,
    /// Steps in the plan
    pub steps: Vec<PlanStepInfo>,
    /// Whether plan has irreversible steps
    pub has_irreversible_steps: bool,
}

/// Step information for UI display (UniFFI compatible)
#[derive(Debug, Clone)]
pub struct PlanStepInfo {
    /// Step index (1-based)
    pub index: u32,
    /// Tool name
    pub tool_name: String,
    /// Step description
    pub description: String,
    /// Safety level label
    pub safety_level: String,
}

/// Plan execution progress (UniFFI compatible)
#[derive(Debug, Clone)]
pub struct PlanProgress {
    /// Plan ID
    pub plan_id: String,
    /// Current step (1-based)
    pub current_step: u32,
    /// Total steps
    pub total_steps: u32,
    /// Current step description
    pub step_description: String,
    /// Step status
    pub status: String,
}

impl PlanProgress {
    /// Create a new progress update
    pub fn new(
        plan_id: impl Into<String>,
        current_step: u32,
        total_steps: u32,
        step_description: impl Into<String>,
        status: StepStatus,
    ) -> Self {
        Self {
            plan_id: plan_id.into(),
            current_step,
            total_steps,
            step_description: step_description.into(),
            status: status.as_str().to_string(),
        }
    }
}

/// Step execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl StepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
        }
    }
}

/// Plan completion result (UniFFI compatible)
#[derive(Debug, Clone)]
pub struct PlanResult {
    /// Plan ID
    pub plan_id: String,
    /// Final output (JSON string)
    pub final_output: String,
    /// Total steps executed
    pub total_steps: u32,
}

/// Plan failure error (UniFFI compatible)
#[derive(Debug, Clone)]
pub struct PlanError {
    /// Plan ID
    pub plan_id: String,
    /// Failed step index (1-based)
    pub failed_step: u32,
    /// Error message
    pub error: String,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_safety_level() {
        assert!(!ToolSafetyLevel::ReadOnly.requires_confirmation());
        assert!(!ToolSafetyLevel::Reversible.requires_confirmation());
        assert!(ToolSafetyLevel::IrreversibleLowRisk.requires_confirmation());
        assert!(ToolSafetyLevel::IrreversibleHighRisk.requires_confirmation());

        assert!(ToolSafetyLevel::ReadOnly.is_reversible());
        assert!(ToolSafetyLevel::Reversible.is_reversible());
        assert!(!ToolSafetyLevel::IrreversibleLowRisk.is_reversible());
        assert!(!ToolSafetyLevel::IrreversibleHighRisk.is_reversible());
    }

    #[test]
    fn test_plan_step_creation() {
        let step = PlanStep::new(1, "search", json!({"query": "test"}), "Search for test")
            .with_safety_level(ToolSafetyLevel::ReadOnly)
            .with_timeout(10_000);

        assert_eq!(step.index, 1);
        assert_eq!(step.tool_name, "search");
        assert_eq!(step.safety_level, ToolSafetyLevel::ReadOnly);
        assert_eq!(step.timeout_ms, 10_000);
        assert!(!step.has_prev_reference());
    }

    #[test]
    fn test_plan_step_prev_reference() {
        let step = PlanStep::new(
            2,
            "summarize",
            json!({"content": "$prev"}),
            "Summarize previous output",
        );

        assert!(step.has_prev_reference());
    }

    #[test]
    fn test_task_plan_creation() {
        let steps = vec![
            PlanStep::new(1, "search", json!({"query": "AI news"}), "Search for AI news"),
            PlanStep::new(
                2,
                "summarize",
                json!({"content": "$prev"}),
                "Summarize results",
            ),
        ];

        let plan = TaskPlan::new("Search and summarize AI news", steps)
            .with_confidence(0.85)
            .with_duration_hint("~5 seconds");

        assert_eq!(plan.step_count(), 2);
        assert!(!plan.is_empty());
        assert_eq!(plan.confidence, 0.85);
        assert!(!plan.has_irreversible_steps);
        assert!(plan.requires_confirmation);
    }

    #[test]
    fn test_task_plan_irreversible_detection() {
        let steps = vec![
            PlanStep::new(1, "read_file", json!({}), "Read file")
                .with_safety_level(ToolSafetyLevel::ReadOnly),
            PlanStep::new(2, "delete_file", json!({}), "Delete file")
                .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk),
        ];

        let plan = TaskPlan::new("Read and delete file", steps);
        assert!(plan.has_irreversible_steps);
    }

    #[test]
    fn test_step_result() {
        let success = StepResult::success(1, json!({"result": "data"}), 150);
        assert!(success.success);
        assert!(success.error.is_none());

        let failure = StepResult::failure(1, "Connection timeout", 5000);
        assert!(!failure.success);
        assert!(failure.error.is_some());
    }

    #[test]
    fn test_plan_execution_context() {
        let steps = vec![
            PlanStep::new(1, "search", json!({}), "Search"),
            PlanStep::new(2, "summarize", json!({}), "Summarize"),
        ];
        let plan = TaskPlan::new("Test plan", steps);
        let mut ctx = PlanExecutionContext::new(plan);

        assert!(!ctx.is_complete());
        assert!(ctx.prev_output().is_none());

        ctx.add_result(StepResult::success(1, json!({"data": "test"}), 100));
        assert!(!ctx.is_complete());
        assert!(ctx.prev_output().is_some());

        ctx.add_result(StepResult::success(2, json!({"summary": "done"}), 200));
        assert!(ctx.is_complete());
    }

    #[test]
    fn test_plan_info_conversion() {
        let steps = vec![PlanStep::new(1, "search", json!({}), "Search the web")];
        let plan = TaskPlan::new("Test plan", steps);
        let info = plan.to_info();

        assert_eq!(info.steps.len(), 1);
        assert_eq!(info.steps[0].tool_name, "search");
        assert!(!info.has_irreversible_steps);
    }

    #[test]
    fn test_safety_level_serialization() {
        let level = ToolSafetyLevel::IrreversibleHighRisk;
        let json = serde_json::to_string(&level).unwrap();
        assert!(json.contains("irreversible_high_risk"));

        let parsed: ToolSafetyLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, level);
    }
}
