//! Integration tests for the POE (Principle-Operation-Evaluation) module.
//!
//! These tests verify the full POE execution cycle, including:
//! - File-based validation constraints
//! - Command execution validation
//! - Budget management and retry logic
//! - Strategy switching on stuck detection

use crate::poe::{
    BudgetStatus, PoeConfig, PoeManager, PoeOutcome, PoeTask, PoeBudget, SuccessManifest,
    ValidationRule, CompositeValidator,
};
use crate::poe::worker::MockWorker;
use crate::providers::MockProvider;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::fs;

// ============================================================================
// Test Utilities
// ============================================================================

fn create_mock_manager(
    worker: MockWorker,
    mock_response: &str,
    config: PoeConfig,
) -> PoeManager<MockWorker> {
    let provider = Arc::new(MockProvider::new(mock_response));
    let validator = CompositeValidator::new(provider);
    PoeManager::new(worker, validator, config)
}

// ============================================================================
// Full POE Cycle Tests
// ============================================================================

/// Test: Full POE cycle with file existence constraint
///
/// Scenario:
/// 1. Create a task requiring a file to exist
/// 2. First, file doesn't exist -> validation fails
/// 3. Worker "creates" the file (simulated by test setup)
/// 4. Validation passes -> Success outcome
#[tokio::test]
async fn test_full_poe_cycle_with_file_constraint() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("output.txt");

    // Create manifest with file existence constraint
    let manifest = SuccessManifest::new("file-task", "Create output.txt")
        .with_hard_constraint(ValidationRule::FileExists {
            path: file_path.clone(),
        })
        .with_max_attempts(3);

    let task = PoeTask::new(manifest, "Create the output file");

    // First attempt: file doesn't exist, should fail
    let worker = MockWorker::new().with_tokens(100);
    let config = PoeConfig::default();
    let manager = create_mock_manager(worker, "", config);

    let outcome = manager.execute(task.clone()).await.unwrap();

    // Should fail because file doesn't exist
    match &outcome {
        PoeOutcome::BudgetExhausted { .. } | PoeOutcome::StrategySwitch { .. } => {
            // Expected - either budget exhausted or stuck
        }
        PoeOutcome::Success(_) => {
            panic!("Should not succeed when file doesn't exist");
        }
    }

    // Now create the file and try again
    fs::write(&file_path, "Hello from POE!").await.unwrap();

    let manifest2 = SuccessManifest::new("file-task-2", "Create output.txt")
        .with_hard_constraint(ValidationRule::FileExists {
            path: file_path.clone(),
        })
        .with_max_attempts(3);

    let task2 = PoeTask::new(manifest2, "Create the output file");
    let worker2 = MockWorker::new().with_tokens(100);
    let manager2 = create_mock_manager(worker2, "", PoeConfig::default());

    let outcome2 = manager2.execute(task2).await.unwrap();

    // Should succeed now
    match outcome2 {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
            assert_eq!(verdict.distance_score, 0.0);
            assert_eq!(verdict.hard_results.len(), 1);
            assert!(verdict.hard_results[0].passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome2),
    }
}

/// Test: POE succeeds immediately when all constraints are met
#[tokio::test]
async fn test_poe_succeeds_when_constraints_met() {
    let dir = tempdir().unwrap();

    // Create required files
    let file1 = dir.path().join("config.json");
    let file2 = dir.path().join("README.md");

    fs::write(&file1, r#"{"version": "1.0"}"#).await.unwrap();
    fs::write(&file2, "# Project README\n\nThis is a test project.")
        .await
        .unwrap();

    let manifest = SuccessManifest::new("multi-file-task", "Create project structure")
        .with_hard_constraint(ValidationRule::FileExists { path: file1 })
        .with_hard_constraint(ValidationRule::FileExists { path: file2 })
        .with_hard_constraint(ValidationRule::FileContains {
            path: dir.path().join("config.json"),
            pattern: r#""version""#.to_string(),
        })
        .with_max_attempts(5);

    let task = PoeTask::new(manifest, "Set up project files");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
            assert_eq!(verdict.hard_results.len(), 3);
            assert!(verdict.hard_results.iter().all(|r| r.passed));
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

/// Test: POE handles command validation
#[tokio::test]
async fn test_poe_command_validation() {
    // Use a command that always succeeds
    let manifest = SuccessManifest::new("cmd-task", "Run echo command")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "echo".to_string(),
            args: vec!["hello".to_string()],
            timeout_ms: 5000,
        })
        .with_max_attempts(2);

    let task = PoeTask::new(manifest, "Execute echo command");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
            assert_eq!(verdict.hard_results.len(), 1);
            assert!(verdict.hard_results[0].passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

/// Test: POE fails on command that returns non-zero exit code
#[tokio::test]
async fn test_poe_command_failure() {
    // Use a command that fails (false returns exit code 1)
    let manifest = SuccessManifest::new("fail-cmd-task", "Run failing command")
        .with_hard_constraint(ValidationRule::CommandPasses {
            cmd: "false".to_string(),
            args: vec![],
            timeout_ms: 5000,
        })
        .with_max_attempts(2);

    let task = PoeTask::new(manifest, "Execute failing command");
    let worker = MockWorker::new().with_tokens(100);
    let manager = create_mock_manager(worker, "", PoeConfig::default().with_stuck_window(3));

    let outcome = manager.execute(task).await.unwrap();

    // Should either exhaust budget or detect stuck
    match outcome {
        PoeOutcome::BudgetExhausted { attempts, .. } => {
            assert_eq!(attempts, 2);
        }
        PoeOutcome::StrategySwitch { .. } => {
            // Also acceptable if stuck detection kicks in
        }
        PoeOutcome::Success(_) => {
            panic!("Should not succeed when command fails");
        }
    }
}

/// Test: Command output contains pattern validation
#[tokio::test]
async fn test_poe_command_output_contains() {
    let manifest = SuccessManifest::new("output-task", "Check command output")
        .with_hard_constraint(ValidationRule::CommandOutputContains {
            cmd: "echo".to_string(),
            args: vec!["hello".to_string(), "world".to_string()],
            pattern: "world".to_string(),
            timeout_ms: 5000,
        })
        .with_max_attempts(2);

    let task = PoeTask::new(manifest, "Verify echo output");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

// ============================================================================
// Budget Management Tests
// ============================================================================

/// Test: Budget exhaustion after max attempts
#[tokio::test]
async fn test_budget_exhausted_after_max_attempts() {
    let manifest = SuccessManifest::new("impossible-task", "Complete impossible task")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("/nonexistent/impossible/path.txt"),
        })
        .with_max_attempts(3);

    let task = PoeTask::new(manifest, "Try the impossible");
    let worker = MockWorker::new().with_tokens(100);
    let config = PoeConfig::default().with_stuck_window(5); // Increase stuck window to avoid strategy switch
    let manager = create_mock_manager(worker, "", config);

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::BudgetExhausted { attempts, .. } => {
            assert_eq!(attempts, 3);
        }
        _ => panic!("Expected BudgetExhausted outcome, got {:?}", outcome),
    }
}

/// Test: Token budget exhaustion
#[tokio::test]
async fn test_token_budget_exhaustion() {
    let manifest = SuccessManifest::new("token-hungry-task", "Consume many tokens")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("/nonexistent.txt"),
        })
        .with_max_attempts(10);

    let task = PoeTask::new(manifest, "Use lots of tokens");
    // Worker consumes 50k tokens per call, budget is 80k
    let worker = MockWorker::new().with_tokens(50_000);
    let config = PoeConfig::default()
        .with_max_tokens(80_000)
        .with_stuck_window(5);
    let manager = create_mock_manager(worker, "", config);

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::BudgetExhausted { attempts, .. } => {
            // Should stop after 2 attempts (50k + 50k >= 80k)
            assert_eq!(attempts, 2);
        }
        _ => panic!("Expected BudgetExhausted outcome, got {:?}", outcome),
    }
}

/// Test: PoeBudget entropy tracking and stuck detection
#[tokio::test]
async fn test_budget_stuck_detection() {
    let mut budget = PoeBudget::new(10, 100_000);

    // Record attempts with same distance (stuck pattern)
    for _ in 0..5 {
        budget.record_attempt(1000, 0.8); // Same distance every time
    }

    assert!(budget.is_stuck(3)); // Stuck over window of 3

    // Budget status should reflect degrading or stuck
    let status = budget.status();
    assert!(matches!(
        status,
        BudgetStatus::Stuck | BudgetStatus::Degrading
    ));
}

/// Test: PoeBudget shows improvement when distance decreases
#[tokio::test]
async fn test_budget_improvement_tracking() {
    let mut budget = PoeBudget::new(10, 100_000);

    // Record improving attempts
    budget.record_attempt(1000, 0.9);
    budget.record_attempt(1000, 0.7);
    budget.record_attempt(1000, 0.5);
    budget.record_attempt(1000, 0.3);

    // Should not be stuck when improving
    assert!(!budget.is_stuck(3));

    // Best score should be tracked
    assert!((budget.best_score().unwrap() - 0.3).abs() < 0.01);
}

// ============================================================================
// Strategy Switch Tests
// ============================================================================

/// Test: Strategy switch when stuck (no progress over window)
#[tokio::test]
async fn test_strategy_switch_on_stuck() {
    let manifest = SuccessManifest::new("stuck-task", "Task that gets stuck")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("/always/fails.txt"),
        })
        .with_max_attempts(10);

    let task = PoeTask::new(manifest, "Try and get stuck");
    let worker = MockWorker::new().with_tokens(100);
    let config = PoeConfig::default().with_stuck_window(3); // Small window for quick stuck detection
    let manager = create_mock_manager(worker, "", config);

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::StrategySwitch { reason, suggestion } => {
            assert!(reason.contains("No progress"));
            assert!(!suggestion.is_empty());
        }
        _ => panic!("Expected StrategySwitch outcome, got {:?}", outcome),
    }
}

// ============================================================================
// File Content Validation Tests
// ============================================================================

/// Test: FileContains validation with regex pattern
#[tokio::test]
async fn test_file_contains_regex_validation() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("source.rs");

    fs::write(
        &file_path,
        r#"
fn main() {
    println!("Hello, World!");
}
"#,
    )
    .await
    .unwrap();

    let manifest = SuccessManifest::new("regex-task", "Check Rust function")
        .with_hard_constraint(ValidationRule::FileContains {
            path: file_path.clone(),
            pattern: r"fn\s+main\(\)".to_string(),
        })
        .with_max_attempts(1);

    let task = PoeTask::new(manifest, "Verify main function exists");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

/// Test: FileNotContains validation
#[tokio::test]
async fn test_file_not_contains_validation() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("safe.rs");

    fs::write(&file_path, "fn safe_function() {}")
        .await
        .unwrap();

    let manifest = SuccessManifest::new("no-unsafe-task", "Check no unsafe code")
        .with_hard_constraint(ValidationRule::FileNotContains {
            path: file_path.clone(),
            pattern: r"unsafe\s*\{".to_string(),
        })
        .with_max_attempts(1);

    let task = PoeTask::new(manifest, "Verify no unsafe blocks");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

// ============================================================================
// Directory Structure Tests
// ============================================================================

/// Test: Directory structure validation
#[tokio::test]
async fn test_dir_structure_validation() {
    let dir = tempdir().unwrap();

    // Create expected structure
    fs::create_dir_all(dir.path().join("src")).await.unwrap();
    fs::create_dir_all(dir.path().join("tests")).await.unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]")
        .await
        .unwrap();

    let manifest = SuccessManifest::new("structure-task", "Verify project structure")
        .with_hard_constraint(ValidationRule::DirStructureMatch {
            root: dir.path().to_path_buf(),
            expected: "src/, tests/, Cargo.toml".to_string(),
        })
        .with_max_attempts(1);

    let task = PoeTask::new(manifest, "Check project layout");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

// ============================================================================
// JSON Schema Validation Tests
// ============================================================================

/// Test: JSON schema validation passes for valid JSON
#[tokio::test]
async fn test_json_schema_validation_passes() {
    let dir = tempdir().unwrap();
    let json_path = dir.path().join("config.json");

    fs::write(
        &json_path,
        r#"{"name": "test", "version": "1.0.0", "count": 42}"#,
    )
    .await
    .unwrap();

    // Note: This validator uses a simplified schema format, not standard JSON Schema.
    // Keys are field names, values are expected types.
    let schema = r#"{
        "name": "string",
        "version": "string",
        "count": "number"
    }"#;

    let manifest = SuccessManifest::new("json-task", "Validate JSON config")
        .with_hard_constraint(ValidationRule::JsonSchemaValid {
            path: json_path,
            schema: schema.to_string(),
        })
        .with_max_attempts(1);

    let task = PoeTask::new(manifest, "Check JSON schema");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

/// Test: JSON schema validation fails for invalid JSON
#[tokio::test]
async fn test_json_schema_validation_fails() {
    let dir = tempdir().unwrap();
    let json_path = dir.path().join("bad.json");

    // Missing required "version" field - validator expects both fields
    fs::write(&json_path, r#"{"name": "test"}"#).await.unwrap();

    // Note: This validator uses a simplified schema format where keys are required field names
    // and values are expected types. Missing keys will cause validation to fail.
    let schema = r#"{
        "name": "string",
        "version": "string"
    }"#;

    let manifest = SuccessManifest::new("invalid-json-task", "Validate invalid JSON")
        .with_hard_constraint(ValidationRule::JsonSchemaValid {
            path: json_path,
            schema: schema.to_string(),
        })
        .with_max_attempts(2);

    let task = PoeTask::new(manifest, "Check JSON schema");
    let worker = MockWorker::new().with_tokens(100);
    let config = PoeConfig::default().with_stuck_window(5);
    let manager = create_mock_manager(worker, "", config);

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::BudgetExhausted { .. } | PoeOutcome::StrategySwitch { .. } => {
            // Expected - validation should fail
        }
        PoeOutcome::Success(_) => {
            panic!("Should not succeed with invalid JSON");
        }
    }
}

// ============================================================================
// Composite Validation Tests
// ============================================================================

/// Test: Multiple validation rules with mixed pass/fail
#[tokio::test]
async fn test_mixed_validation_rules() {
    let dir = tempdir().unwrap();
    let existing_file = dir.path().join("exists.txt");
    fs::write(&existing_file, "content").await.unwrap();

    // One rule passes (file exists), one fails (file doesn't exist)
    let manifest = SuccessManifest::new("mixed-task", "Test mixed validation")
        .with_hard_constraint(ValidationRule::FileExists {
            path: existing_file,
        })
        .with_hard_constraint(ValidationRule::FileExists {
            path: dir.path().join("missing.txt"),
        })
        .with_max_attempts(2);

    let task = PoeTask::new(manifest, "Check files");
    let worker = MockWorker::new().with_tokens(100);
    let config = PoeConfig::default().with_stuck_window(5);
    let manager = create_mock_manager(worker, "", config);

    let outcome = manager.execute(task).await.unwrap();

    // Overall should fail because one hard constraint fails
    match outcome {
        PoeOutcome::BudgetExhausted { .. } | PoeOutcome::StrategySwitch { .. } => {
            // Expected
        }
        PoeOutcome::Success(verdict) => {
            // Should have failed hard results
            assert!(
                !verdict.passed || verdict.hard_results.iter().any(|r| !r.passed),
                "Should not fully succeed with failing constraint"
            );
        }
    }
}

// ============================================================================
// Worker Execution Count Tests
// ============================================================================

/// Test: Worker is called correct number of times
#[tokio::test]
async fn test_worker_execution_count() {
    let manifest = SuccessManifest::new("count-task", "Count executions")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("/nonexistent.txt"),
        })
        .with_max_attempts(4);

    let task = PoeTask::new(manifest, "Count worker calls");
    let worker = MockWorker::new().with_tokens(100);
    let config = PoeConfig::default().with_stuck_window(10); // High window to avoid early exit
    let manager = create_mock_manager(worker, "", config);

    let _ = manager.execute(task).await.unwrap();

    // Worker should have been called exactly 4 times (max_attempts)
    assert_eq!(manager.worker().execution_count(), 4);
}

// ============================================================================
// Edge Cases
// ============================================================================

/// Test: Empty manifest (no constraints) passes immediately
#[tokio::test]
async fn test_empty_manifest_passes() {
    let manifest = SuccessManifest::new("empty-task", "No constraints");
    let task = PoeTask::new(manifest, "Do nothing");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(verdict) => {
            assert!(verdict.passed);
            assert_eq!(verdict.distance_score, 0.0);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}

/// Test: Single attempt with immediate success
#[tokio::test]
async fn test_single_attempt_success() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("quick.txt");
    fs::write(&file_path, "quick success").await.unwrap();

    let manifest = SuccessManifest::new("quick-task", "Quick success")
        .with_hard_constraint(ValidationRule::FileExists { path: file_path })
        .with_max_attempts(1);

    let task = PoeTask::new(manifest, "Succeed quickly");
    let worker = MockWorker::new();
    let manager = create_mock_manager(worker, "", PoeConfig::default());

    let outcome = manager.execute(task).await.unwrap();

    match outcome {
        PoeOutcome::Success(_) => {
            // Worker should only be called once
            assert_eq!(manager.worker().execution_count(), 1);
        }
        _ => panic!("Expected Success outcome, got {:?}", outcome),
    }
}
