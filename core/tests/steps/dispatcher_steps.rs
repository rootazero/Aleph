//! Step definitions for dispatcher cortex features
//!
//! Covers security pipeline, JSON stream parsing, decision flow, and DAG scheduling.

use crate::world::{AlephWorld, DispatcherContext};
// TODO: removed — module deleted: use alephcore::dispatcher::cortex::{...};
use alephcore::dispatcher::{DagTaskDisplayStatus, RiskLevel, UserDecision};
use cucumber::{given, then, when};
use std::sync::atomic::Ordering;

// TODO: removed — dispatcher::cortex module deleted
// Security Pipeline Steps, JSON Stream Parsing Steps, and Decision Flow Steps
// have been removed because they depend on alephcore::dispatcher::cortex
// which no longer exists in the codebase.

// ═══════════════════════════════════════════════════════════════════════════
// Risk Evaluator Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a risk evaluator")]
async fn given_risk_evaluator(w: &mut AlephWorld) {
    let ctx = w.dispatcher.get_or_insert_with(DispatcherContext::default);
    ctx.create_risk_evaluator();
}

#[when(expr = "I evaluate an AI task {string} with prompt {string}")]
async fn when_evaluate_ai_task(w: &mut AlephWorld, name: String, prompt: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    let task = DispatcherContext::create_ai_task("t1", &name, &prompt);
    ctx.evaluate_task_risk(&task);
}

#[when(expr = "I evaluate a code task {string} with code {string}")]
async fn when_evaluate_code_task(w: &mut AlephWorld, name: String, code: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    let task = DispatcherContext::create_code_task("t1", &name, &code);
    ctx.evaluate_task_risk(&task);
}

#[then(expr = "the risk level should be {string}")]
async fn then_risk_level_should_be(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let risk_level = ctx.last_risk_level.as_ref().expect("No risk level");
    let expected_level = match expected.as_str() {
        "low" => RiskLevel::Low,
        "high" => RiskLevel::High,
        _ => panic!("Unknown risk level: {}", expected),
    };
    assert_eq!(
        *risk_level, expected_level,
        "Risk level should be {}",
        expected
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Task Context Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a task context with user input {string}")]
async fn given_task_context(w: &mut AlephWorld, user_input: String) {
    let ctx = w.dispatcher.get_or_insert_with(DispatcherContext::default);
    ctx.create_task_context(&user_input);
}

#[given(expr = "task {string} has output {string}")]
async fn given_task_has_output(w: &mut AlephWorld, task_id: String, output: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.record_task_output(&task_id, &output);
}

#[given(expr = "task {string} named {string} has output {string}")]
async fn given_task_named_has_output(
    w: &mut AlephWorld,
    task_id: String,
    name: String,
    output: String,
) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.record_task_output_with_name(&task_id, &name, &output);
}

#[when(expr = "I build prompt context for {string} with no dependencies")]
async fn when_build_prompt_no_deps(w: &mut AlephWorld, task_id: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.build_prompt_context(&task_id, &[]);
}

#[when(expr = "I build prompt context for {string} depending on {string}")]
async fn when_build_prompt_with_dep(w: &mut AlephWorld, task_id: String, dep: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.build_prompt_context(&task_id, &[&dep]);
}

#[when(expr = "task {string} has output {string}")]
async fn when_task_has_output(w: &mut AlephWorld, task_id: String, output: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.record_task_output(&task_id, &output);
}

#[then(expr = "the prompt context should contain {string}")]
async fn then_prompt_context_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let prompt = ctx.last_prompt_context.as_ref().expect("No prompt context");
    assert!(
        prompt.contains(&expected),
        "Prompt context '{}' should contain '{}'",
        prompt,
        expected
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Task Graph Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a task graph {string} titled {string}")]
async fn given_task_graph(w: &mut AlephWorld, id: String, title: String) {
    let ctx = w.dispatcher.get_or_insert_with(DispatcherContext::default);
    ctx.create_task_graph(&id, &title);
}

#[given(expr = "the graph has AI task {string} named {string}")]
async fn given_graph_has_ai_task(w: &mut AlephWorld, id: String, name: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.add_ai_task_to_graph(&id, &name);
}

#[given(expr = "the graph has code task {string} named {string} with code {string}")]
async fn given_graph_has_code_task(w: &mut AlephWorld, id: String, name: String, code: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.add_code_task_to_graph(&id, &name, &code);
}

#[given(expr = "task {string} depends on {string}")]
async fn given_task_depends_on(w: &mut AlephWorld, from: String, to: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.add_graph_dependency(&from, &to);
}

#[when("I create a task plan without confirmation required")]
async fn when_create_plan_no_confirm(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.create_task_plan(false);
}

#[when("I create a task plan with confirmation required")]
async fn when_create_plan_with_confirm(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.create_task_plan(true);
}

#[when("I create a task plan with high risk flag")]
async fn when_create_plan_with_risk_flag(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.create_task_plan_with_risk();
}

#[when("I evaluate the graph for risk")]
async fn when_evaluate_graph_risk(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.evaluate_graph_risk();
}

#[then(expr = "the plan should have id {string}")]
async fn then_plan_has_id(w: &mut AlephWorld, expected_id: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.as_ref().expect("No task plan");
    assert_eq!(plan.id, expected_id, "Plan id should be {}", expected_id);
}

#[then(expr = "the plan should have title {string}")]
async fn then_plan_has_title(w: &mut AlephWorld, expected_title: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.as_ref().expect("No task plan");
    assert_eq!(
        plan.title, expected_title,
        "Plan title should be {}",
        expected_title
    );
}

#[then(expr = "the plan should have {int} tasks")]
async fn then_plan_has_n_tasks(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.as_ref().expect("No task plan");
    assert_eq!(
        plan.task_count(),
        expected as usize,
        "Plan should have {} tasks",
        expected
    );
}

#[then("the plan should not require confirmation")]
async fn then_plan_no_confirm(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.as_ref().expect("No task plan");
    assert!(
        !plan.requires_confirmation,
        "Plan should not require confirmation"
    );
}

#[then("the plan should require confirmation")]
async fn then_plan_requires_confirm(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.as_ref().expect("No task plan");
    assert!(
        plan.requires_confirmation,
        "Plan should require confirmation"
    );
}

#[then("the plan should have high risk tasks")]
async fn then_plan_has_high_risk(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.as_ref().expect("No task plan");
    assert!(
        plan.has_high_risk_tasks(),
        "Plan should have high risk tasks"
    );
}

#[then("the graph should have high risk")]
async fn then_graph_has_high_risk(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let high_risk = ctx.graph_has_high_risk.expect("Graph risk not evaluated");
    assert!(high_risk, "Graph should have high risk");
}

// ═══════════════════════════════════════════════════════════════════════════
// Task Info Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a task info {string} named {string} with status {string} and risk {string}")]
async fn given_task_info(
    w: &mut AlephWorld,
    id: String,
    name: String,
    status: String,
    risk: String,
) {
    let ctx = w.dispatcher.get_or_insert_with(DispatcherContext::default);
    let status_enum = match status.as_str() {
        "pending" => DagTaskDisplayStatus::Pending,
        "running" => DagTaskDisplayStatus::Running,
        "completed" => DagTaskDisplayStatus::Completed,
        "failed" => DagTaskDisplayStatus::Failed,
        "cancelled" => DagTaskDisplayStatus::Cancelled,
        _ => panic!("Unknown status: {}", status),
    };
    let risk_enum = match risk.as_str() {
        "low" => RiskLevel::Low,
        "high" => RiskLevel::High,
        _ => panic!("Unknown risk: {}", risk),
    };
    ctx.create_task_info(&id, &name, status_enum, risk_enum);
}

#[given(expr = "the task info has dependency {string}")]
async fn given_task_info_dependency(w: &mut AlephWorld, dep: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.add_task_info_dependency(&dep);
}

#[then(expr = "the task info id should be {string}")]
async fn then_task_info_id(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let info = ctx.task_info.as_ref().expect("No task info");
    assert_eq!(info.id, expected, "Task info id should be {}", expected);
}

#[then(expr = "the task info name should be {string}")]
async fn then_task_info_name(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let info = ctx.task_info.as_ref().expect("No task info");
    assert_eq!(info.name, expected, "Task info name should be {}", expected);
}

#[then(expr = "the task info status should be {string}")]
async fn then_task_info_status(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let info = ctx.task_info.as_ref().expect("No task info");
    let expected_status = match expected.as_str() {
        "pending" => DagTaskDisplayStatus::Pending,
        "running" => DagTaskDisplayStatus::Running,
        "completed" => DagTaskDisplayStatus::Completed,
        "failed" => DagTaskDisplayStatus::Failed,
        "cancelled" => DagTaskDisplayStatus::Cancelled,
        _ => panic!("Unknown status: {}", expected),
    };
    assert_eq!(
        info.status, expected_status,
        "Task info status should be {}",
        expected
    );
}

#[then(expr = "the task info risk level should be {string}")]
async fn then_task_info_risk_level(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let info = ctx.task_info.as_ref().expect("No task info");
    assert_eq!(
        info.risk_level, expected,
        "Task info risk level should be {}",
        expected
    );
}

#[then(expr = "the task info should have dependency {string}")]
async fn then_task_info_has_dependency(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let info = ctx.task_info.as_ref().expect("No task info");
    assert!(
        info.dependencies.contains(&expected),
        "Task info should have dependency {}",
        expected
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Task Display Status Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a task display status {word}")]
async fn given_task_display_status(w: &mut AlephWorld, status: String) {
    let ctx = w.dispatcher.get_or_insert_with(DispatcherContext::default);
    let status_enum = match status.as_str() {
        "pending" => DagTaskDisplayStatus::Pending,
        "running" => DagTaskDisplayStatus::Running,
        "completed" => DagTaskDisplayStatus::Completed,
        "failed" => DagTaskDisplayStatus::Failed,
        "cancelled" => DagTaskDisplayStatus::Cancelled,
        _ => panic!("Unknown status: {}", status),
    };
    ctx.task_display_status = Some(status_enum);
}

#[then(expr = "the status string should be {string}")]
async fn then_status_string(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let status = ctx
        .task_display_status
        .as_ref()
        .expect("No task display status");
    assert_eq!(
        status.to_string(),
        expected,
        "Status string should be {}",
        expected
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// NoOp Callback Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a no-op execution callback")]
async fn given_noop_callback(w: &mut AlephWorld) {
    let ctx = w.dispatcher.get_or_insert_with(DispatcherContext::default);
    ctx.noop_callback_completed = false;
    ctx.noop_confirmation_result = None;
}

#[given(expr = "an empty task plan {string} titled {string}")]
async fn given_empty_task_plan(w: &mut AlephWorld, id: String, title: String) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.create_empty_task_plan(&id, &title);
}

#[when("I call all callback methods")]
async fn when_call_all_callback_methods(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    let plan = ctx.task_plan.clone().expect("No task plan");
    ctx.test_noop_callback(&plan).await;
}

#[then("all callback methods should complete without error")]
async fn then_all_callbacks_complete(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    assert!(
        ctx.noop_callback_completed,
        "All callback methods should complete"
    );
}

#[then(expr = "confirmation should return {string}")]
async fn then_confirmation_returns(w: &mut AlephWorld, expected: String) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let result = ctx
        .noop_confirmation_result
        .as_ref()
        .expect("No confirmation result");
    let expected_decision = match expected.as_str() {
        "confirmed" => UserDecision::Confirmed,
        "cancelled" => UserDecision::Cancelled,
        _ => panic!("Unknown decision: {}", expected),
    };
    assert_eq!(
        *result, expected_decision,
        "Confirmation should return {}",
        expected
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DAG Scheduler Execution Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a mock task executor")]
async fn given_mock_executor(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.create_mock_executor();
}

#[given("a collecting callback")]
async fn given_collecting_callback(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    ctx.create_collecting_callback();
}

#[when("I execute the graph")]
async fn when_execute_graph(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_mut()
        .expect("Dispatcher context not initialized");
    let result = ctx.execute_graph().await;
    if let Err(e) = result {
        w.last_error = Some(e.to_string());
    }
}

#[then("the DAG execution should succeed")]
async fn then_dag_execution_succeeds(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    assert!(
        ctx.execution_result.is_some(),
        "DAG execution should succeed. Error: {:?}",
        w.last_error
    );
}

#[then(expr = "{int} tasks should be executed")]
async fn then_n_tasks_executed(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let executor = ctx.mock_executor.as_ref().expect("No mock executor");
    assert_eq!(
        executor.get_execution_count(),
        expected as usize,
        "Expected {} tasks executed",
        expected
    );
}

#[then(expr = "{int} tasks should be completed")]
async fn then_n_tasks_completed(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    assert_eq!(
        result.completed_tasks.len(),
        expected as usize,
        "Expected {} tasks completed",
        expected
    );
}

#[then(expr = "{int} tasks should have failed")]
async fn then_n_tasks_failed(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    assert_eq!(
        result.failed_tasks.len(),
        expected as usize,
        "Expected {} tasks failed",
        expected
    );
}

#[then("the execution should not be cancelled")]
async fn then_execution_not_cancelled(w: &mut AlephWorld) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    assert!(!result.cancelled, "Execution should not be cancelled");
}

#[then(expr = "plan_ready callback should be called {int} time")]
async fn then_plan_ready_called_n(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let callback = ctx
        .collecting_callback
        .as_ref()
        .expect("No collecting callback");
    assert_eq!(
        callback.plan_ready_count.load(Ordering::SeqCst),
        expected as usize,
        "plan_ready should be called {} time(s)",
        expected
    );
}

#[then(expr = "task_start callback should be called {int} times")]
async fn then_task_start_called_n(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let callback = ctx
        .collecting_callback
        .as_ref()
        .expect("No collecting callback");
    assert_eq!(
        callback.task_start_count.load(Ordering::SeqCst),
        expected as usize,
        "task_start should be called {} time(s)",
        expected
    );
}

#[then(expr = "task_complete callback should be called {int} times")]
async fn then_task_complete_called_n(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let callback = ctx
        .collecting_callback
        .as_ref()
        .expect("No collecting callback");
    assert_eq!(
        callback.task_complete_count.load(Ordering::SeqCst),
        expected as usize,
        "task_complete should be called {} time(s)",
        expected
    );
}

#[then(expr = "all_complete callback should be called {int} time")]
async fn then_all_complete_called_n(w: &mut AlephWorld, expected: i32) {
    let ctx = w
        .dispatcher
        .as_ref()
        .expect("Dispatcher context not initialized");
    let callback = ctx
        .collecting_callback
        .as_ref()
        .expect("No collecting callback");
    assert_eq!(
        callback.all_complete_count.load(Ordering::SeqCst),
        expected as usize,
        "all_complete should be called {} time(s)",
        expected
    );
}
