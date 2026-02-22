//! Recovery Tests for Multi-Agent Resilience Module
//!
//! Tests crash recovery scenarios including:
//! - Graceful shutdown handling
//! - Interrupted task recovery
//! - Shadow Replay restoration
//! - Risk-aware recovery decisions

use crate::resilience::*;
use crate::resilience::database::StateDatabase;
use std::sync::Arc;
use tempfile::TempDir;

/// Test helper to create a temporary database
fn create_test_db() -> (TempDir, Arc<StateDatabase>) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(StateDatabase::new(db_path).expect("Failed to create database"));
    (temp_dir, db)
}

// =============================================================================
// Graceful Shutdown Tests
// =============================================================================

#[tokio::test]
async fn test_graceful_shutdown_subscription() {
    let (_temp, db) = create_test_db();
    let shutdown = GracefulShutdown::new(db);

    // Subscribe
    let mut rx = shutdown.subscribe();

    // Trigger shutdown
    shutdown.trigger(ShutdownSignal::Requested);

    // Should receive signal
    let signal = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
        .await
        .expect("Timeout waiting for signal")
        .expect("Channel closed");

    assert!(matches!(signal, ShutdownSignal::Requested));
}

#[tokio::test]
async fn test_graceful_shutdown_multiple_subscribers() {
    let (_temp, db) = create_test_db();
    let shutdown = GracefulShutdown::new(db);

    let mut rx1 = shutdown.subscribe();
    let mut rx2 = shutdown.subscribe();
    let mut rx3 = shutdown.subscribe();

    shutdown.trigger(ShutdownSignal::Requested);

    // All subscribers should receive signal
    assert!(rx1.recv().await.is_ok());
    assert!(rx2.recv().await.is_ok());
    assert!(rx3.recv().await.is_ok());
}

// =============================================================================
// Interrupted Task Recovery Tests
// =============================================================================

#[tokio::test]
async fn test_mark_running_as_interrupted() {
    let (_temp, db) = create_test_db();

    // Create tasks in various states
    let task1 = AgentTask::new("task_r1", "sess", "agent", "Running task 1", RiskLevel::Low);
    let task2 = AgentTask::new("task_r2", "sess", "agent", "Running task 2", RiskLevel::High);
    let task3 = AgentTask::new("task_c1", "sess", "agent", "Completed task", RiskLevel::Low);

    db.insert_agent_task(&task1).await.expect("Failed");
    db.insert_agent_task(&task2).await.expect("Failed");
    db.insert_agent_task(&task3).await.expect("Failed");

    // Set states
    db.update_task_status("task_r1", TaskStatus::Running)
        .await
        .expect("Failed");
    db.update_task_status("task_r2", TaskStatus::Running)
        .await
        .expect("Failed");
    db.update_task_status("task_c1", TaskStatus::Completed)
        .await
        .expect("Failed");

    // Mark running as interrupted
    let count = db
        .mark_running_as_interrupted()
        .await
        .expect("Failed to mark");

    assert_eq!(count, 2);

    // Verify states
    let t1 = db.get_agent_task("task_r1").await.expect("Failed").unwrap();
    let t2 = db.get_agent_task("task_r2").await.expect("Failed").unwrap();
    let t3 = db.get_agent_task("task_c1").await.expect("Failed").unwrap();

    assert_eq!(t1.status, TaskStatus::Interrupted);
    assert_eq!(t2.status, TaskStatus::Interrupted);
    assert_eq!(t3.status, TaskStatus::Completed); // Unchanged
}

#[tokio::test]
async fn test_get_recoverable_tasks() {
    let (_temp, db) = create_test_db();

    // Create interrupted tasks
    let task1 = AgentTask::new("task_i1", "sess", "agent", "Interrupted low risk", RiskLevel::Low);
    let task2 = AgentTask::new("task_i2", "sess", "agent", "Interrupted high risk", RiskLevel::High);
    let task3 = AgentTask::new("task_p1", "sess", "agent", "Pending task", RiskLevel::Low);

    db.insert_agent_task(&task1).await.expect("Failed");
    db.insert_agent_task(&task2).await.expect("Failed");
    db.insert_agent_task(&task3).await.expect("Failed");

    db.update_task_status("task_i1", TaskStatus::Interrupted)
        .await
        .expect("Failed");
    db.update_task_status("task_i2", TaskStatus::Interrupted)
        .await
        .expect("Failed");

    // Get recoverable tasks
    let recoverable = db
        .get_recoverable_tasks()
        .await
        .expect("Failed to get recoverable");

    assert_eq!(recoverable.len(), 2);
    assert!(recoverable.iter().all(|t| t.status == TaskStatus::Interrupted));
}

// =============================================================================
// Recovery Manager Tests
// =============================================================================

#[tokio::test]
async fn test_recovery_manager_scan() {
    let (_temp, db) = create_test_db();

    // Create interrupted tasks with different risk levels
    let low_risk = AgentTask::new("task_low", "sess", "agent", "Search query", RiskLevel::Low);
    let high_risk = AgentTask::new("task_high", "sess", "agent", "Write file", RiskLevel::High);

    db.insert_agent_task(&low_risk).await.expect("Failed");
    db.insert_agent_task(&high_risk).await.expect("Failed");

    db.update_task_status("task_low", TaskStatus::Interrupted)
        .await
        .expect("Failed");
    db.update_task_status("task_high", TaskStatus::Interrupted)
        .await
        .expect("Failed");

    // Add traces so tasks are recoverable
    let trace1 = TaskTrace::new("task_low", 0, TraceRole::Assistant, r#"{"content":"test"}"#);
    let trace2 = TaskTrace::new("task_high", 0, TraceRole::Assistant, r#"{"content":"test"}"#);
    db.insert_trace(&trace1).await.expect("Failed");
    db.insert_trace(&trace2).await.expect("Failed");

    // Create recovery manager
    let manager = RecoveryManager::new(db.clone());

    // Scan for recoverable tasks
    let decisions = manager
        .scan_recoverable_tasks()
        .await
        .expect("Failed to scan");

    assert_eq!(decisions.len(), 2);

    // Check decisions by iterating
    let mut has_auto_resume = false;
    let mut has_pending = false;
    for decision in &decisions {
        match decision {
            RecoveryDecision::AutoResume { task, .. } if task.id == "task_low" => {
                has_auto_resume = true;
            }
            RecoveryDecision::PendingConfirmation { task } if task.id == "task_high" => {
                has_pending = true;
            }
            _ => {}
        }
    }
    assert!(has_auto_resume, "Low risk task should auto-resume");
    assert!(has_pending, "High risk task should need confirmation");
}

#[tokio::test]
async fn test_recovery_summary() {
    let (_temp, db) = create_test_db();

    // Create various interrupted tasks with traces
    for i in 0..3 {
        let task = AgentTask::new(
            &format!("task_low_{}", i),
            "sess",
            "agent",
            "Low risk task",
            RiskLevel::Low,
        );
        db.insert_agent_task(&task).await.expect("Failed");
        db.update_task_status(&format!("task_low_{}", i), TaskStatus::Interrupted)
            .await
            .expect("Failed");
        // Add trace so task is recoverable
        let trace = TaskTrace::new(&format!("task_low_{}", i), 0, TraceRole::Assistant, "{}");
        db.insert_trace(&trace).await.expect("Failed");
    }

    for i in 0..2 {
        let task = AgentTask::new(
            &format!("task_high_{}", i),
            "sess",
            "agent",
            "High risk task",
            RiskLevel::High,
        );
        db.insert_agent_task(&task).await.expect("Failed");
        db.update_task_status(&format!("task_high_{}", i), TaskStatus::Interrupted)
            .await
            .expect("Failed");
        // Add trace so task is recoverable
        let trace = TaskTrace::new(&format!("task_high_{}", i), 0, TraceRole::Assistant, "{}");
        db.insert_trace(&trace).await.expect("Failed");
    }

    let manager = RecoveryManager::new(db.clone());

    let summary = manager
        .get_recovery_summary()
        .await
        .expect("Failed to get summary");

    assert_eq!(summary.total_count, 5);
    assert_eq!(summary.auto_resume_count, 3);
    assert_eq!(summary.pending_confirmation_count, 2);
}

// =============================================================================
// Shadow Replay Tests
// =============================================================================

#[tokio::test]
async fn test_shadow_replay_empty_task() {
    let (_temp, db) = create_test_db();

    let task = AgentTask::new("task_empty", "sess", "agent", "Empty task", RiskLevel::Low);
    db.insert_agent_task(&task).await.expect("Failed");

    let engine = ShadowReplayEngine::new(db.clone());

    let result = engine
        .replay_task("task_empty")
        .await
        .expect("Failed to replay");

    assert!(result.messages.is_empty());
    assert_eq!(result.last_step, 0);
    assert!(result.complete);
}

#[tokio::test]
async fn test_shadow_replay_with_traces() {
    let (_temp, db) = create_test_db();

    let task = AgentTask::new("task_replay", "sess", "agent", "Replay test", RiskLevel::Low);
    db.insert_agent_task(&task).await.expect("Failed");

    // Add traces
    let trace1 = TaskTrace::new(
        "task_replay",
        0,
        TraceRole::Assistant,
        r#"{"content":"Let me search for that."}"#,
    );
    let trace2 = TaskTrace::new(
        "task_replay",
        1,
        TraceRole::Tool,
        r#"{"tool_call_id":"call_1","result":"Found 3 results"}"#,
    );
    let trace3 = TaskTrace::new(
        "task_replay",
        2,
        TraceRole::Assistant,
        r#"{"content":"Based on the search results..."}"#,
    );

    db.insert_trace(&trace1).await.expect("Failed");
    db.insert_trace(&trace2).await.expect("Failed");
    db.insert_trace(&trace3).await.expect("Failed");

    let engine = ShadowReplayEngine::new(db.clone());

    let result = engine
        .replay_task("task_replay")
        .await
        .expect("Failed to replay");

    assert_eq!(result.messages.len(), 3);
    assert_eq!(result.last_step, 2);
    assert!(result.complete);
}

#[tokio::test]
async fn test_shadow_replay_until_step() {
    let (_temp, db) = create_test_db();

    let task = AgentTask::new("task_partial", "sess", "agent", "Partial replay", RiskLevel::Low);
    db.insert_agent_task(&task).await.expect("Failed");

    // Add 5 traces
    for i in 0..5 {
        let trace = TaskTrace::new(
            "task_partial",
            i,
            TraceRole::Assistant,
            format!(r#"{{"step":{}}}"#, i),
        );
        db.insert_trace(&trace).await.expect("Failed");
    }

    let engine = ShadowReplayEngine::new(db.clone());

    // Replay until step 2
    let result = engine
        .replay_until_step("task_partial", 2)
        .await
        .expect("Failed to replay");

    assert_eq!(result.messages.len(), 3); // Steps 0, 1, 2
    assert_eq!(result.last_step, 2);
    assert!(!result.complete);
}

// =============================================================================
// Risk Adapter Tests
// =============================================================================

#[test]
fn test_risk_adapter_evaluation() {
    let adapter = TaskRiskAdapter::new();

    // Low risk prompts
    assert_eq!(
        adapter.evaluate_prompt("Search for information about Rust"),
        RiskLevel::Low
    );
    assert_eq!(
        adapter.evaluate_prompt("Analyze this code"),
        RiskLevel::Low
    );
    assert_eq!(
        adapter.evaluate_prompt("Read the file contents"),
        RiskLevel::Low
    );

    // High risk prompts
    assert_eq!(
        adapter.evaluate_prompt("Write the changes to file.rs"),
        RiskLevel::High
    );
    assert_eq!(
        adapter.evaluate_prompt("Delete the old files"),
        RiskLevel::High
    );
    assert_eq!(
        adapter.evaluate_prompt("Execute the bash command"),
        RiskLevel::High
    );
    assert_eq!(
        adapter.evaluate_prompt("Send message to user"),
        RiskLevel::High
    );
}

#[test]
fn test_risk_adapter_tools() {
    let adapter = TaskRiskAdapter::new();

    // Low risk tools
    assert_eq!(
        adapter.evaluate_tools(&["search", "read_file", "list_files"]),
        RiskLevel::Low
    );

    // High risk tools
    assert_eq!(
        adapter.evaluate_tools(&["write_file"]),
        RiskLevel::High
    );
    assert_eq!(
        adapter.evaluate_tools(&["bash"]),
        RiskLevel::High
    );
    assert_eq!(
        adapter.evaluate_tools(&["search", "write_file"]),
        RiskLevel::High
    );
}

// =============================================================================
// Divergence Detection Tests
// =============================================================================

#[tokio::test]
async fn test_divergence_detection_reached_end() {
    let (_temp, db) = create_test_db();

    let task = AgentTask::new("task_div", "sess", "agent", "Divergence test", RiskLevel::Low);
    db.insert_agent_task(&task).await.expect("Failed");

    // Add only one trace
    let trace = TaskTrace::new("task_div", 0, TraceRole::Assistant, r#"{"content":"Done"}"#);
    db.insert_trace(&trace).await.expect("Failed");

    let engine = ShadowReplayEngine::new(db.clone());

    // Check divergence at step 0 (no next step)
    let status = engine
        .check_divergence("task_div", 0, None)
        .await
        .expect("Failed to check");

    assert_eq!(status, DivergenceStatus::ReachedEnd);
}
