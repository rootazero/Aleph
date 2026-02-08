//! Governance Tests for Multi-Agent Resilience Module
//!
//! Tests resource governance including:
//! - Lane-based priority isolation
//! - Quota enforcement
//! - Recursion depth limiting
//! - Resource permits

use crate::memory::database::resilience::*;
use crate::memory::database::VectorDatabase;
use std::sync::Arc;
use tempfile::TempDir;

/// Test helper to create a temporary database
fn create_test_db() -> (TempDir, Arc<VectorDatabase>) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).expect("Failed to create database"));
    (temp_dir, db)
}

// =============================================================================
// Resource Governor Tests
// =============================================================================

#[tokio::test]
async fn test_governor_acquire_release() {
    let (_temp, db) = create_test_db();

    let governor = ResourceGovernor::new(db.clone());

    // Acquire main lane resource
    let permit = governor
        .acquire(Lane::Main)
        .await
        .expect("Failed to acquire");

    assert_eq!(permit.lane(), Lane::Main);

    let stats = governor.get_stats();
    assert_eq!(stats.main_lane_active, 1);

    // Release
    governor.release(permit);

    let stats_after = governor.get_stats();
    assert_eq!(stats_after.main_lane_active, 0);
}

#[tokio::test]
async fn test_governor_try_acquire() {
    let (_temp, db) = create_test_db();

    let config = GovernorConfig {
        max_running_subagents: 2,
        ..Default::default()
    };

    let governor = ResourceGovernor::with_config(db.clone(), config);

    // Try acquire should succeed
    let permit1 = governor.try_acquire(Lane::Subagent);
    assert!(permit1.is_some());

    let permit2 = governor.try_acquire(Lane::Subagent);
    assert!(permit2.is_some());

    // Third should fail (capacity reached)
    let permit3 = governor.try_acquire(Lane::Subagent);
    assert!(permit3.is_none());

    // Release one
    governor.release(permit1.unwrap());

    // Now should succeed
    let permit4 = governor.try_acquire(Lane::Subagent);
    assert!(permit4.is_some());
}

#[tokio::test]
async fn test_governor_has_capacity() {
    let (_temp, db) = create_test_db();

    let config = GovernorConfig {
        max_running_subagents: 1,
        ..Default::default()
    };

    let governor = ResourceGovernor::with_config(db.clone(), config);

    assert!(governor.has_capacity(Lane::Subagent));

    let permit = governor
        .acquire(Lane::Subagent)
        .await
        .expect("Failed to acquire");

    assert!(!governor.has_capacity(Lane::Subagent));

    governor.release(permit);

    assert!(governor.has_capacity(Lane::Subagent));
}

#[tokio::test]
async fn test_governor_token_tracking() {
    let (_temp, db) = create_test_db();

    let config = GovernorConfig {
        token_budget_per_session: 1000,
        ..Default::default()
    };

    let governor = ResourceGovernor::with_config(db.clone(), config);

    // Record tokens within budget
    let ok = governor
        .record_tokens("session_1", 500)
        .await
        .expect("Failed to record");
    assert!(ok);

    // Check usage
    let usage = governor.get_token_usage("session_1").await;
    assert_eq!(usage, 500);

    // Record more tokens
    let ok = governor
        .record_tokens("session_1", 400)
        .await
        .expect("Failed to record");
    assert!(ok);

    // Exceed budget
    let ok = governor
        .record_tokens("session_1", 200)
        .await
        .expect("Failed to record");
    assert!(!ok); // Budget exceeded

    // Reset
    governor.reset_session_tokens("session_1").await;
    let usage_after = governor.get_token_usage("session_1").await;
    assert_eq!(usage_after, 0);
}

#[test]
fn test_governor_stats() {
    let stats = GovernorStats {
        main_lane_active: 1,
        main_lane_capacity: 2,
        subagent_lane_active: 3,
        subagent_lane_capacity: 5,
    };

    assert_eq!(stats.total_active(), 4);
    assert!((stats.main_lane_utilization() - 0.5).abs() < 0.001);
    assert!((stats.subagent_lane_utilization() - 0.6).abs() < 0.001);
}

// =============================================================================
// Recursive Sentry Tests
// =============================================================================

#[test]
fn test_sentry_check_depth() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).expect("Failed to create database"));

    let sentry = RecursiveSentry::new(db, 3);

    // Within limit
    assert!(sentry.check_depth(0).is_ok());
    assert!(sentry.check_depth(1).is_ok());
    assert!(sentry.check_depth(2).is_ok());
    assert!(sentry.check_depth(3).is_ok());

    // Exceeds limit
    assert!(sentry.check_depth(4).is_err());
    assert!(sentry.check_depth(10).is_err());
}

#[test]
fn test_sentry_remaining_depth() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).expect("Failed to create database"));

    let sentry = RecursiveSentry::new(db, 3);

    assert_eq!(sentry.remaining_depth(0), 3);
    assert_eq!(sentry.remaining_depth(1), 2);
    assert_eq!(sentry.remaining_depth(2), 1);
    assert_eq!(sentry.remaining_depth(3), 0);
    assert_eq!(sentry.remaining_depth(5), 0); // Saturates at 0
}

#[test]
fn test_sentry_is_near_limit() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).expect("Failed to create database"));

    let sentry = RecursiveSentry::new(db, 3);

    assert!(!sentry.is_near_limit(0));
    assert!(!sentry.is_near_limit(1));
    assert!(sentry.is_near_limit(2)); // max - 1
    assert!(sentry.is_near_limit(3)); // at max
}

#[tokio::test]
async fn test_sentry_calculate_child_depth() {
    let (_temp, db) = create_test_db();

    // Create parent task with depth 1
    let mut parent = AgentTask::new("parent_task", "sess", "agent", "Parent", RiskLevel::Low);
    parent.recursion_depth = 1;
    db.insert_agent_task(&parent).await.expect("Failed");

    let sentry = RecursiveSentry::new(db.clone(), 3);

    // Child of parent should be depth 2
    let child_depth = sentry
        .calculate_child_depth(Some("parent_task"))
        .await
        .expect("Failed to calculate");

    assert_eq!(child_depth, 2);

    // Root task (no parent) should be depth 0
    let root_depth = sentry
        .calculate_child_depth(None)
        .await
        .expect("Failed to calculate");

    assert_eq!(root_depth, 0);
}

#[tokio::test]
async fn test_sentry_validate_spawn() {
    let (_temp, db) = create_test_db();

    // Create task at depth 2
    let mut task = AgentTask::new("deep_task", "sess", "agent", "Deep", RiskLevel::Low);
    task.recursion_depth = 2;
    db.insert_agent_task(&task).await.expect("Failed");

    let sentry = RecursiveSentry::new(db.clone(), 3);

    // Spawning child of depth-2 task should succeed (child will be depth 3)
    let result = sentry.validate_spawn(Some("deep_task")).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 3);

    // Create task at depth 3
    let mut max_task = AgentTask::new("max_task", "sess", "agent", "Max", RiskLevel::Low);
    max_task.recursion_depth = 3;
    db.insert_agent_task(&max_task).await.expect("Failed");

    // Spawning child of depth-3 task should fail (child would be depth 4)
    let result = sentry.validate_spawn(Some("max_task")).await;
    assert!(result.is_err());
}

// =============================================================================
// Quota Manager Tests
// =============================================================================

#[test]
fn test_quota_check_tokens() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).expect("Failed to create database"));

    let config = QuotaConfig {
        token_budget: 1000,
        ..Default::default()
    };

    let manager = QuotaManager::with_config(db, config);

    assert!(manager.check_tokens(500).is_ok());
    assert!(manager.check_tokens(1000).is_ok());
    assert!(manager.check_tokens(1001).is_err());
}

#[test]
fn test_quota_check_tool_calls() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db = Arc::new(VectorDatabase::new(db_path).expect("Failed to create database"));

    let config = QuotaConfig {
        max_tool_calls_per_task: 10,
        ..Default::default()
    };

    let manager = QuotaManager::with_config(db, config);

    assert!(manager.check_tool_calls(5).is_ok());
    assert!(manager.check_tool_calls(9).is_ok());
    assert!(manager.check_tool_calls(10).is_err());
    assert!(manager.check_tool_calls(15).is_err());
}

#[tokio::test]
async fn test_quota_check_spawn() {
    let (_temp, db) = create_test_db();

    let config = QuotaConfig {
        max_running: 2,
        max_depth: 3,
        ..Default::default()
    };

    let manager = QuotaManager::with_config(db.clone(), config);

    // No sessions yet, should pass
    let result = manager.check_spawn(0).await.expect("Failed to check");
    assert!(result.passed);

    // Create sessions to fill capacity
    let sess1 = SubagentSession::new("sess_1", "explorer", "parent");
    let sess2 = SubagentSession::new("sess_2", "explorer", "parent");
    db.insert_session(&sess1).await.expect("Failed");
    db.insert_session(&sess2).await.expect("Failed");

    // Now at capacity, should fail
    let result = manager.check_spawn(0).await.expect("Failed to check");
    assert!(!result.passed);
    assert!(result
        .violations
        .iter()
        .any(|v| matches!(v, QuotaViolation::MaxRunningExceeded { .. })));

    // Check depth violation
    let result = manager.check_spawn(5).await.expect("Failed to check");
    assert!(!result.passed);
    assert!(result
        .violations
        .iter()
        .any(|v| matches!(v, QuotaViolation::MaxDepthExceeded { .. })));
}

#[tokio::test]
async fn test_quota_remaining_capacity() {
    let (_temp, db) = create_test_db();

    let config = QuotaConfig {
        max_running: 5,
        max_idle: 10,
        max_total: 20,
        ..Default::default()
    };

    let manager = QuotaManager::with_config(db.clone(), config);

    // Initially full capacity
    let capacity = manager
        .get_remaining_capacity()
        .await
        .expect("Failed to get capacity");

    assert_eq!(capacity.running, 5);
    assert_eq!(capacity.idle, 10);
    assert_eq!(capacity.total, 20);
    assert!(capacity.has_capacity());

    // Add some sessions
    for i in 0..3 {
        let sess = SubagentSession::new(&format!("sess_{}", i), "explorer", "parent");
        db.insert_session(&sess).await.expect("Failed");
    }

    let capacity = manager
        .get_remaining_capacity()
        .await
        .expect("Failed to get capacity");

    assert_eq!(capacity.running, 2); // 5 - 3
    assert_eq!(capacity.total, 17); // 20 - 3
}

// =============================================================================
// Quota Violation Tests
// =============================================================================

#[test]
fn test_quota_violation_display() {
    let violations = vec![
        QuotaViolation::MaxRunningExceeded {
            current: 6,
            limit: 5,
        },
        QuotaViolation::MaxIdleExceeded {
            current: 12,
            limit: 10,
        },
        QuotaViolation::MaxDepthExceeded {
            current: 4,
            limit: 3,
        },
        QuotaViolation::TokenBudgetExceeded {
            used: 150000,
            budget: 100000,
        },
        QuotaViolation::MaxTotalExceeded {
            current: 55,
            limit: 50,
        },
        QuotaViolation::MaxToolCallsExceeded {
            current: 110,
            limit: 100,
        },
    ];

    for violation in violations {
        let msg = violation.to_string();
        assert!(!msg.is_empty());
        // Verify the message contains relevant numbers
        match &violation {
            QuotaViolation::MaxRunningExceeded { current, limit } => {
                assert!(msg.contains(&current.to_string()));
                assert!(msg.contains(&limit.to_string()));
            }
            _ => {}
        }
    }
}

// =============================================================================
// Lane Tests
// =============================================================================

#[test]
fn test_lane_enum() {
    assert_eq!(Lane::Main.to_string(), "main");
    assert_eq!(Lane::Subagent.to_string(), "subagent");

    assert_eq!(Lane::from_str_or_default("main"), Lane::Main);
    assert_eq!(Lane::from_str_or_default("subagent"), Lane::Subagent);
    assert_eq!(Lane::from_str_or_default("unknown"), Lane::Subagent); // Default
}
