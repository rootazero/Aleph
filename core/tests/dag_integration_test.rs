//! Integration tests for DAG scheduler integration
//!
//! Tests the RiskEvaluator, TaskContext, and DagScheduler
//! working together for multi-step task execution.

use aethecore::dispatcher::{
    DagTaskDisplayStatus, DagTaskInfo, DagTaskPlan, ExecutionCallback, NoOpExecutionCallback,
    RiskEvaluator, RiskLevel, TaskContext, TaskOutput, UserDecision,
};
use aethecore::dispatcher::agent_types::{AiTask, CodeExec, Language, Task, TaskGraph, TaskType};
use aethecore::dispatcher::scheduler::DagScheduler;
use aethecore::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// MARK: - Test Helpers

/// Simple mock executor for testing
struct MockTaskExecutor {
    execution_count: AtomicUsize,
}

impl MockTaskExecutor {
    fn new() -> Self {
        Self {
            execution_count: AtomicUsize::new(0),
        }
    }

    fn get_execution_count(&self) -> usize {
        self.execution_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl aethecore::dispatcher::scheduler::GraphTaskExecutor for MockTaskExecutor {
    async fn execute(
        &self,
        task: &Task,
        _context: &str,
    ) -> Result<TaskOutput> {
        self.execution_count.fetch_add(1, Ordering::SeqCst);
        Ok(TaskOutput::text(format!("Executed: {}", task.name)))
    }
}

/// Collecting callback that records all events
struct CollectingCallback {
    plan_ready_count: AtomicUsize,
    task_start_count: AtomicUsize,
    task_complete_count: AtomicUsize,
    all_complete_count: AtomicUsize,
}

impl CollectingCallback {
    fn new() -> Self {
        Self {
            plan_ready_count: AtomicUsize::new(0),
            task_start_count: AtomicUsize::new(0),
            task_complete_count: AtomicUsize::new(0),
            all_complete_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ExecutionCallback for CollectingCallback {
    async fn on_plan_ready(&self, _plan: &DagTaskPlan) {
        self.plan_ready_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_confirmation_required(&self, _plan: &DagTaskPlan) -> UserDecision {
        UserDecision::Confirmed
    }

    async fn on_task_start(&self, _task_id: &str, _task_name: &str) {
        self.task_start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_task_stream(&self, _task_id: &str, _chunk: &str) {}

    async fn on_task_complete(&self, _task_id: &str, _summary: &str) {
        self.task_complete_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_task_retry(&self, _task_id: &str, _attempt: u32, _error: &str) {}

    async fn on_task_deciding(&self, _task_id: &str, _error: &str) {}

    async fn on_task_failed(&self, _task_id: &str, _error: &str) {}

    async fn on_all_complete(&self, _summary: &str) {
        self.all_complete_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_cancelled(&self) {}
}

/// Create a simple AI task
fn ai_task(id: &str, name: &str, prompt: &str) -> Task {
    Task::new(
        id,
        name,
        TaskType::AiInference(AiTask {
            prompt: prompt.to_string(),
            requires_privacy: false,
            has_images: false,
            output_format: None,
        }),
    )
}

/// Create a code execution task
fn code_task(id: &str, name: &str, code: &str) -> Task {
    Task::new(
        id,
        name,
        TaskType::CodeExecution(CodeExec::Script {
            code: code.to_string(),
            language: Language::Python,
        }),
    )
}

// MARK: - RiskEvaluator Tests

#[test]
fn test_risk_evaluator_low_risk_patterns() {
    let evaluator = RiskEvaluator::new();

    // Pure AI inference task - should be low risk
    let task = ai_task("t1", "Analyze text", "Analyze: {input}");
    assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);
}

#[test]
fn test_risk_evaluator_high_risk_code_execution() {
    let evaluator = RiskEvaluator::new();

    // Code execution task - should be high risk
    let task = code_task("t1", "Execute shell command", "echo test");
    assert_eq!(evaluator.evaluate(&task), RiskLevel::High);
}

#[test]
fn test_risk_evaluator_high_risk_keywords() {
    let evaluator = RiskEvaluator::new();

    // Task with high-risk keywords in name
    let api_task = ai_task("t1", "Call external API to fetch data", "Fetch data");
    assert_eq!(evaluator.evaluate(&api_task), RiskLevel::High);

    let http_task = ai_task("t2", "Make HTTP request to server", "Request data");
    assert_eq!(evaluator.evaluate(&http_task), RiskLevel::High);
}

// MARK: - TaskContext Tests

#[test]
fn test_task_context_creation() {
    let context = TaskContext::new("User wants to analyze document");

    // Initial prompt context should contain user input
    let prompt = context.build_prompt_context("task_1", &[]);
    assert!(prompt.contains("User wants to analyze document"));
}

#[test]
fn test_task_context_record_output() {
    let mut context = TaskContext::new("Test input");

    // Record first task output
    context.record_output("task_1", TaskOutput::text("First result"));

    // Second task should see first task's result when depending on it
    let prompt = context.build_prompt_context("task_2", &["task_1"]);
    assert!(prompt.contains("First result"));
}

#[test]
fn test_task_context_explicit_reference() {
    let mut context = TaskContext::new("Test input");

    // Record output with named variable
    context.record_output_with_name("task_1", "Analysis Task", TaskOutput::text("Analysis complete"));

    // Build context - should include the name
    let prompt = context.build_prompt_context("task_2", &["task_1"]);
    assert!(prompt.contains("Analysis complete"));
}

// MARK: - DagTaskPlan Tests

#[test]
fn test_task_plan_from_graph() {
    let mut graph = TaskGraph::new("plan_1", "Test Plan");

    graph.add_task(ai_task("t1", "First task", "Task 1"));
    graph.add_task(ai_task("t2", "Second task", "Task 2"));
    graph.add_dependency("t1", "t2");

    let plan = DagTaskPlan::from_graph(&graph, false);

    assert_eq!(plan.id, "plan_1");
    assert_eq!(plan.title, "Test Plan");
    assert_eq!(plan.task_count(), 2);
    assert!(!plan.requires_confirmation);
}

#[test]
fn test_task_plan_high_risk_detection() {
    let mut graph = TaskGraph::new("plan_1", "High Risk Plan");

    // Add a code execution task (high risk)
    graph.add_task(code_task("t1", "Run code", "print('hello')"));

    let plan = DagTaskPlan::from_graph(&graph, true);

    assert!(plan.has_high_risk_tasks());
    assert!(plan.requires_confirmation);
}

// MARK: - DagTaskInfo Tests

#[test]
fn test_task_info_creation() {
    let info = DagTaskInfo::new(
        "task_1",
        "Read file",
        DagTaskDisplayStatus::Pending,
        RiskLevel::Low,
        vec!["task_0".to_string()],
    );

    assert_eq!(info.id, "task_1");
    assert_eq!(info.name, "Read file");
    assert_eq!(info.status, DagTaskDisplayStatus::Pending);
    assert_eq!(info.risk_level, "low");
    assert_eq!(info.dependencies, vec!["task_0"]);
}

#[test]
fn test_task_info_high_risk() {
    let info = DagTaskInfo::new(
        "task_1",
        "Execute command",
        DagTaskDisplayStatus::Running,
        RiskLevel::High,
        vec![],
    );

    assert_eq!(info.risk_level, "high");
}

// MARK: - DagTaskDisplayStatus Tests

#[test]
fn test_task_display_status_display() {
    assert_eq!(DagTaskDisplayStatus::Pending.to_string(), "pending");
    assert_eq!(DagTaskDisplayStatus::Running.to_string(), "running");
    assert_eq!(DagTaskDisplayStatus::Completed.to_string(), "completed");
    assert_eq!(DagTaskDisplayStatus::Failed.to_string(), "failed");
    assert_eq!(DagTaskDisplayStatus::Cancelled.to_string(), "cancelled");
}

// MARK: - NoOpCallback Tests

#[tokio::test]
async fn test_noop_callback_all_methods() {
    let callback = NoOpExecutionCallback;
    let plan = DagTaskPlan {
        id: "test".to_string(),
        title: "Test Plan".to_string(),
        tasks: vec![],
        requires_confirmation: false,
    };

    // All methods should complete without error
    callback.on_plan_ready(&plan).await;
    assert_eq!(
        callback.on_confirmation_required(&plan).await,
        UserDecision::Confirmed
    );
    callback.on_task_start("t1", "Task 1").await;
    callback.on_task_stream("t1", "output").await;
    callback.on_task_complete("t1", "done").await;
    callback.on_task_retry("t1", 1, "error").await;
    callback.on_task_deciding("t1", "error").await;
    callback.on_task_failed("t1", "error").await;
    callback.on_all_complete("done").await;
    callback.on_cancelled().await;
}

// MARK: - DagScheduler Integration Tests

#[tokio::test]
async fn test_dag_scheduler_linear_execution() {
    // Create a linear DAG: t1 -> t2 -> t3
    let mut graph = TaskGraph::new("linear_plan", "Linear Execution");

    graph.add_task(ai_task("t1", "Task 1", "Task 1"));
    graph.add_task(ai_task("t2", "Task 2", "Task 2"));
    graph.add_task(ai_task("t3", "Task 3", "Task 3"));

    graph.add_dependency("t1", "t2");
    graph.add_dependency("t2", "t3");

    let executor = Arc::new(MockTaskExecutor::new());
    let callback = Arc::new(CollectingCallback::new());
    let context = TaskContext::new("Test input");

    let result = DagScheduler::execute_graph(
        graph,
        executor.clone(),
        callback.clone(),
        context,
        None,
    )
    .await;

    assert!(result.is_ok());
    let exec_result = result.unwrap();

    // All 3 tasks should be executed
    assert_eq!(executor.get_execution_count(), 3);
    assert_eq!(exec_result.completed_tasks.len(), 3);
    assert!(exec_result.failed_tasks.is_empty());
    assert!(!exec_result.cancelled);

    // Callback should have been called correctly
    assert_eq!(callback.plan_ready_count.load(Ordering::SeqCst), 1);
    assert_eq!(callback.task_start_count.load(Ordering::SeqCst), 3);
    assert_eq!(callback.task_complete_count.load(Ordering::SeqCst), 3);
    assert_eq!(callback.all_complete_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_dag_scheduler_parallel_execution() {
    // Create a parallel DAG: t1 -> (t2, t3) -> t4
    //   t1
    //  /  \
    // t2  t3
    //  \  /
    //   t4
    let mut graph = TaskGraph::new("parallel_plan", "Parallel Execution");

    graph.add_task(ai_task("t1", "Task 1", "Task 1"));
    graph.add_task(ai_task("t2", "Task 2", "Task 2"));
    graph.add_task(ai_task("t3", "Task 3", "Task 3"));
    graph.add_task(ai_task("t4", "Task 4", "Task 4"));

    graph.add_dependency("t1", "t2");
    graph.add_dependency("t1", "t3");
    graph.add_dependency("t2", "t4");
    graph.add_dependency("t3", "t4");

    let executor = Arc::new(MockTaskExecutor::new());
    let callback = Arc::new(CollectingCallback::new());
    let context = TaskContext::new("Test input");

    let result = DagScheduler::execute_graph(
        graph,
        executor.clone(),
        callback.clone(),
        context,
        None,
    )
    .await;

    assert!(result.is_ok());
    let exec_result = result.unwrap();

    // All 4 tasks should be executed
    assert_eq!(executor.get_execution_count(), 4);
    assert_eq!(exec_result.completed_tasks.len(), 4);
}

// MARK: - End-to-End Tests

#[test]
fn test_full_workflow_risk_evaluation() {
    // Create a graph with mixed risk tasks
    let mut graph = TaskGraph::new("mixed_risk", "Mixed Risk Plan");

    // Low risk: AI inference
    graph.add_task(ai_task("t1", "Analyze document", "Analyze"));

    // High risk: Code execution
    graph.add_task(code_task("t2", "Execute analysis script", "analyze()"));

    graph.add_dependency("t1", "t2");

    let evaluator = RiskEvaluator::new();
    let has_high_risk = evaluator.evaluate_graph(&graph);

    // Graph should be flagged as high risk
    assert!(has_high_risk);

    // Create plan with confirmation required
    let plan = DagTaskPlan::from_graph(&graph, has_high_risk);
    assert!(plan.requires_confirmation);
    assert!(plan.has_high_risk_tasks());
}

#[test]
fn test_context_propagation_through_tasks() {
    let mut context = TaskContext::new("Analyze the following text and summarize");

    // Simulate task execution flow
    context.record_output("analysis", TaskOutput::text("The text discusses AI safety"));
    context.record_output_with_name("analysis", "Document Analysis", TaskOutput::text("The text discusses AI safety"));

    // Next task should have access to previous output when depending on it
    let prompt = context.build_prompt_context("summarize", &["analysis"]);
    assert!(prompt.contains("AI safety"));

    // Third task builds on previous
    context.record_output("summarize", TaskOutput::text("Summary: AI safety is important"));
    let final_prompt = context.build_prompt_context("generate_report", &["summarize"]);
    assert!(final_prompt.contains("Summary"));
}
