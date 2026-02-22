//! Integration Tests for Multi-Agent Resilience Module
//!
//! Tests the complete workflow of the resilience system including:
//! - Task lifecycle management
//! - Trace recording and replay
//! - Event emission and observation
//! - Session coordination
//! - Resource governance

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
// Task Lifecycle Tests
// =============================================================================

#[tokio::test]
async fn test_task_lifecycle_complete() {
    let (_temp, db) = create_test_db();

    // Create a task
    let task = AgentTask::new(
        "task_001",
        "session_001",
        "agent_001",
        "Search for information about Rust",
        RiskLevel::Low,
    );

    // Insert task
    db.insert_agent_task(&task).await.expect("Failed to insert task");

    // Verify task exists
    let retrieved = db
        .get_agent_task("task_001")
        .await
        .expect("Failed to get task")
        .expect("Task not found");

    assert_eq!(retrieved.id, "task_001");
    assert_eq!(retrieved.status, TaskStatus::Pending);
    assert_eq!(retrieved.risk_level, RiskLevel::Low);

    // Update to running
    db.update_task_status("task_001", TaskStatus::Running)
        .await
        .expect("Failed to update status");

    let running = db
        .get_agent_task("task_001")
        .await
        .expect("Failed to get task")
        .expect("Task not found");

    assert_eq!(running.status, TaskStatus::Running);
    assert!(running.started_at.is_some());

    // Complete the task
    db.update_task_status("task_001", TaskStatus::Completed)
        .await
        .expect("Failed to update status");

    let completed = db
        .get_agent_task("task_001")
        .await
        .expect("Failed to get task")
        .expect("Task not found");

    assert_eq!(completed.status, TaskStatus::Completed);
    assert!(completed.completed_at.is_some());
}

#[tokio::test]
async fn test_task_with_traces() {
    let (_temp, db) = create_test_db();

    // Create task
    let task = AgentTask::new(
        "task_002",
        "session_001",
        "agent_001",
        "Analyze code",
        RiskLevel::Low,
    );
    db.insert_agent_task(&task).await.expect("Failed to insert task");

    // Record traces
    let trace1 = TaskTrace::new("task_002", 0, TraceRole::Assistant, r#"{"content":"Let me analyze..."}"#);
    let trace2 = TaskTrace::new("task_002", 1, TraceRole::Tool, r#"{"tool_call_id":"call_1","result":"Found 5 files"}"#);
    let trace3 = TaskTrace::new("task_002", 2, TraceRole::Assistant, r#"{"content":"Based on the analysis..."}"#);

    db.insert_trace(&trace1).await.expect("Failed to insert trace");
    db.insert_trace(&trace2).await.expect("Failed to insert trace");
    db.insert_trace(&trace3).await.expect("Failed to insert trace");

    // Retrieve traces
    let traces = db
        .get_traces_by_task("task_002")
        .await
        .expect("Failed to get traces");

    assert_eq!(traces.len(), 3);
    assert_eq!(traces[0].step_index, 0);
    assert_eq!(traces[1].step_index, 1);
    assert_eq!(traces[2].step_index, 2);

    // Get trace count
    let count = db
        .get_trace_count("task_002")
        .await
        .expect("Failed to get count");

    assert_eq!(count, 3);
}

#[tokio::test]
async fn test_bulk_trace_insert() {
    let (_temp, db) = create_test_db();

    // Create task
    let task = AgentTask::new(
        "task_003",
        "session_001",
        "agent_001",
        "Long running task",
        RiskLevel::Low,
    );
    db.insert_agent_task(&task).await.expect("Failed to insert task");

    // Bulk insert traces
    let traces: Vec<TaskTrace> = (0..100)
        .map(|i| TaskTrace::new("task_003", i, TraceRole::Assistant, format!(r#"{{"step":{}}}"#, i)))
        .collect();

    db.bulk_insert_traces(&traces)
        .await
        .expect("Failed to bulk insert");

    let count = db
        .get_trace_count("task_003")
        .await
        .expect("Failed to get count");

    assert_eq!(count, 100);
}

// =============================================================================
// Event System Tests
// =============================================================================

#[tokio::test]
async fn test_event_emission_and_retrieval() {
    let (_temp, db) = create_test_db();

    // Create task first
    let task = AgentTask::new(
        "task_004",
        "session_001",
        "agent_001",
        "Event test task",
        RiskLevel::Low,
    );
    db.insert_agent_task(&task).await.expect("Failed to insert task");

    // Emit events
    let event1 = AgentEvent::structural("task_004", 1, "task_started", r#"{"task_id":"task_004"}"#);
    let event2 = AgentEvent::structural("task_004", 2, "tool_call_started", r#"{"tool":"search"}"#);
    let event3 = AgentEvent::pulse("task_004", 3, "ai_streaming", r#"{"chunk":"Hello"}"#);
    let event4 = AgentEvent::structural("task_004", 4, "tool_call_completed", r#"{"result":"done"}"#);

    db.insert_event(&event1).await.expect("Failed to insert event");
    db.insert_event(&event2).await.expect("Failed to insert event");
    db.insert_event(&event3).await.expect("Failed to insert event");
    db.insert_event(&event4).await.expect("Failed to insert event");

    // Get all events
    let events = db
        .get_events_by_task("task_004")
        .await
        .expect("Failed to get events");

    assert_eq!(events.len(), 4);

    // Get structural events only
    let structural = db
        .get_structural_events("task_004")
        .await
        .expect("Failed to get structural events");

    assert_eq!(structural.len(), 3);
    assert!(structural.iter().all(|e| e.is_structural));

    // Get events in range
    let range = db
        .get_events_in_range("task_004", 2, 3)
        .await
        .expect("Failed to get range");

    assert_eq!(range.len(), 2);
}

#[tokio::test]
async fn test_event_sequence_tracking() {
    let (_temp, db) = create_test_db();

    // Create task
    let task = AgentTask::new(
        "task_005",
        "session_001",
        "agent_001",
        "Sequence test",
        RiskLevel::Low,
    );
    db.insert_agent_task(&task).await.expect("Failed to insert task");

    // Insert events with gaps
    db.insert_event(&AgentEvent::structural("task_005", 1, "event", "{}"))
        .await
        .expect("Failed");
    db.insert_event(&AgentEvent::structural("task_005", 2, "event", "{}"))
        .await
        .expect("Failed");
    db.insert_event(&AgentEvent::structural("task_005", 5, "event", "{}"))
        .await
        .expect("Failed");

    // Get latest seq
    let latest = db
        .get_latest_event_seq("task_005")
        .await
        .expect("Failed to get latest");

    assert_eq!(latest, Some(5));

    // Get events since seq 2 (exclusive, so only seq 5)
    let since = db
        .get_events_since_seq("task_005", 2)
        .await
        .expect("Failed to get since");

    assert_eq!(since.len(), 1); // only seq 5 (seq > 2)
}

// =============================================================================
// Session Management Tests
// =============================================================================

#[tokio::test]
async fn test_session_lifecycle() {
    let (_temp, db) = create_test_db();

    // Create session
    let session = SubagentSession::new("sess_001", "explorer", "parent_001");
    db.insert_session(&session)
        .await
        .expect("Failed to insert session");

    // Verify session
    let retrieved = db
        .get_session("sess_001")
        .await
        .expect("Failed to get session")
        .expect("Session not found");

    assert_eq!(retrieved.id, "sess_001");
    assert_eq!(retrieved.agent_type, "explorer");
    assert_eq!(retrieved.status, SessionStatus::Active);

    // Update to idle
    db.update_session_status("sess_001", SessionStatus::Idle, None)
        .await
        .expect("Failed to update");

    let idle = db
        .get_session("sess_001")
        .await
        .expect("Failed to get session")
        .expect("Session not found");

    assert_eq!(idle.status, SessionStatus::Idle);

    // Update usage
    db.update_session_usage("sess_001", 1000, 10)
        .await
        .expect("Failed to update usage");

    let updated = db
        .get_session("sess_001")
        .await
        .expect("Failed to get session")
        .expect("Session not found");

    assert_eq!(updated.total_tokens_used, 1000);
    assert_eq!(updated.total_tool_calls, 10);
}

#[tokio::test]
async fn test_session_counting() {
    let (_temp, db) = create_test_db();

    // Create multiple sessions
    for i in 0..5 {
        let session = SubagentSession::new(&format!("sess_{}", i), "explorer", "parent_001");
        db.insert_session(&session).await.expect("Failed to insert");
    }

    // Count active
    let active_count = db
        .count_sessions_by_status(SessionStatus::Active)
        .await
        .expect("Failed to count");

    assert_eq!(active_count, 5);

    // Mark some as idle
    db.update_session_status("sess_0", SessionStatus::Idle, None)
        .await
        .expect("Failed");
    db.update_session_status("sess_1", SessionStatus::Idle, None)
        .await
        .expect("Failed");

    let idle_count = db
        .count_sessions_by_status(SessionStatus::Idle)
        .await
        .expect("Failed to count");

    assert_eq!(idle_count, 2);

    // Get idle sessions
    let idle_sessions = db.get_idle_sessions(10).await.expect("Failed to get idle");
    assert_eq!(idle_sessions.len(), 2);
}

// =============================================================================
// Event Classification Tests
// =============================================================================

#[test]
fn test_event_classification() {
    // Skeleton events
    assert_eq!(
        EventClassifier::classify(&EventType::TaskStarted),
        EventTier::Skeleton
    );
    assert_eq!(
        EventClassifier::classify(&EventType::ToolCallCompleted),
        EventTier::Skeleton
    );
    assert_eq!(
        EventClassifier::classify(&EventType::ArtifactCreated),
        EventTier::Skeleton
    );

    // Pulse events
    assert_eq!(
        EventClassifier::classify(&EventType::AiStreamingChunk),
        EventTier::Pulse
    );
    assert_eq!(
        EventClassifier::classify(&EventType::ProgressUpdate),
        EventTier::Pulse
    );

    // Volatile events
    assert_eq!(
        EventClassifier::classify(&EventType::Heartbeat),
        EventTier::Volatile
    );
    assert_eq!(
        EventClassifier::classify(&EventType::MetricsSnapshot),
        EventTier::Volatile
    );

    // Custom events default to Skeleton
    assert_eq!(
        EventClassifier::classify(&EventType::Custom("unknown".to_string())),
        EventTier::Skeleton
    );
}

#[test]
fn test_pulse_buffer() {
    let mut buffer = PulseBuffer::with_config(3, 10000);

    let event = AgentEvent::pulse("task", 1, "streaming", "{}");

    // First two events don't trigger flush
    assert!(!buffer.push(event.clone()));
    assert!(!buffer.push(event.clone()));

    // Third event triggers flush
    assert!(buffer.push(event));

    // Drain and verify
    let events = buffer.drain();
    assert_eq!(events.len(), 3);
    assert!(buffer.is_empty());
}

// =============================================================================
// Coordinator Tests
// =============================================================================

#[tokio::test]
async fn test_session_coordinator_basic() {
    let (_temp, db) = create_test_db();

    let coordinator = SessionCoordinator::new(db.clone());

    // Create session
    let handle = coordinator
        .create_session("explorer", "parent_001")
        .await
        .expect("Failed to create session");

    assert!(handle.is_idle().await);

    // Get counts
    let counts = coordinator
        .get_session_counts()
        .await
        .expect("Failed to get counts");

    assert_eq!(counts.active, 1);
    assert_eq!(counts.idle, 0);

    // Release session
    coordinator
        .release_session(handle.session_id())
        .await
        .expect("Failed to release");

    let counts_after = coordinator
        .get_session_counts()
        .await
        .expect("Failed to get counts");

    assert_eq!(counts_after.idle, 1);
}

#[tokio::test]
async fn test_session_reuse() {
    let (_temp, db) = create_test_db();

    let coordinator = SessionCoordinator::new(db.clone());

    // Create and release a session
    let handle1 = coordinator
        .create_session("explorer", "parent_001")
        .await
        .expect("Failed to create");

    let session_id = handle1.session_id().to_string();

    coordinator
        .release_session(&session_id)
        .await
        .expect("Failed to release");

    // Acquire should reuse the idle session
    let handle2 = coordinator
        .acquire_session("explorer", "parent_001")
        .await
        .expect("Failed to acquire");

    assert_eq!(handle2.session_id(), session_id);
}
