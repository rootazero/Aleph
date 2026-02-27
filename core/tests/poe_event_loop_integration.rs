//! Integration test for POE event loop round-trip.
//!
//! Verifies: PoeEventBus -> PoeManager.execute() -> events received in correct order.
//!
//! Since `MockWorker` is `#[cfg(test)]` only (internal to the crate), we define
//! simple inline Worker implementations for integration testing.

use std::path::PathBuf;
use std::sync::Arc;

use alephcore::poe::{
    CompositeValidator, PoeConfig, PoeEvent, PoeEventBus, PoeManager, PoeOutcome, PoeOutcomeKind,
    PoeTask, SuccessManifest, ValidationRule, WorkerOutput, WorkerState,
};
use alephcore::poe::worker::{StateSnapshot, Worker};
use alephcore::providers::MockProvider;

// ============================================================================
// Inline Worker implementations (MockWorker is cfg(test)-gated in core)
// ============================================================================

/// Worker that creates a file on execution, satisfying a FileExists constraint.
struct FileCreatingWorker {
    workspace: PathBuf,
}

#[async_trait::async_trait]
impl Worker for FileCreatingWorker {
    async fn execute(
        &self,
        _instruction: &str,
        _previous_failure: Option<&str>,
    ) -> alephcore::Result<WorkerOutput> {
        // Create the file that the validation rule expects
        let file = self.workspace.join("test_output.txt");
        std::fs::write(&file, "test output content").map_err(|e| {
            alephcore::AlephError::other(format!("Failed to write test file: {}", e))
        })?;

        Ok(WorkerOutput {
            tokens_consumed: 500,
            steps_taken: 1,
            final_state: WorkerState::Completed {
                summary: "Created test_output.txt".into(),
            },
            artifacts: vec![],
            execution_log: vec![],
        })
    }

    async fn abort(&self) -> alephcore::Result<()> {
        Ok(())
    }

    async fn snapshot(&self) -> alephcore::Result<StateSnapshot> {
        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> alephcore::Result<()> {
        Ok(())
    }
}

/// Worker that always completes but never produces the right file (for failure testing).
struct AlwaysFailingWorker;

#[async_trait::async_trait]
impl Worker for AlwaysFailingWorker {
    async fn execute(
        &self,
        _instruction: &str,
        _previous_failure: Option<&str>,
    ) -> alephcore::Result<WorkerOutput> {
        Ok(WorkerOutput {
            tokens_consumed: 1000,
            steps_taken: 1,
            final_state: WorkerState::Completed {
                summary: "I did something but not the right thing".into(),
            },
            artifacts: vec![],
            execution_log: vec![],
        })
    }

    async fn abort(&self) -> alephcore::Result<()> {
        Ok(())
    }

    async fn snapshot(&self) -> alephcore::Result<StateSnapshot> {
        Ok(StateSnapshot::new("/tmp/test".into()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> alephcore::Result<()> {
        Ok(())
    }
}

// ============================================================================
// Helper: build a CompositeValidator with a MockProvider
// ============================================================================

fn make_validator() -> CompositeValidator {
    let provider = Arc::new(MockProvider::new(""));
    CompositeValidator::new(provider)
}

// ============================================================================
// Tests
// ============================================================================

/// Full round-trip: success path.
///
/// Expect events: ManifestCreated -> OperationAttempted -> ValidationCompleted -> OutcomeRecorded
#[tokio::test]
async fn test_poe_event_loop_success_round_trip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();

    let worker = FileCreatingWorker {
        workspace: tmp.path().to_path_buf(),
    };

    let validator = make_validator();
    let config = PoeConfig::default();
    let manager = PoeManager::new(worker, validator, config).with_event_bus(bus.clone());

    // Create task with a hard constraint that our worker satisfies
    let manifest = SuccessManifest::new("event-test-1", "Create test output")
        .with_hard_constraint(ValidationRule::FileExists {
            path: tmp.path().join("test_output.txt"),
        });
    let task = PoeTask::new(manifest, "Create a file called test_output.txt");

    // Execute
    let outcome = manager.execute(task).await.unwrap();
    assert!(
        matches!(outcome, PoeOutcome::Success(_)),
        "Expected Success, got {:?}",
        outcome
    );

    // Collect events (drain with timeout to avoid hanging)
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(envelope)) => events.push(envelope),
            _ => break,
        }
    }

    // Verify 4-event sequence
    assert!(
        events.len() >= 4,
        "Should have at least 4 events, got {}",
        events.len()
    );

    let tags: Vec<&str> = events.iter().map(|e| e.event_type_tag()).collect();
    assert_eq!(tags[0], "ManifestCreated", "First event");
    assert_eq!(tags[1], "OperationAttempted", "Second event");
    assert_eq!(tags[2], "ValidationCompleted", "Third event");
    assert_eq!(tags[3], "OutcomeRecorded", "Fourth event");

    // Verify ManifestCreated payload
    if let PoeEvent::ManifestCreated {
        task_id, objective, ..
    } = &events[0].event
    {
        assert_eq!(task_id, "event-test-1");
        assert_eq!(objective, "Create test output");
    } else {
        panic!("Expected ManifestCreated, got {:?}", events[0].event);
    }

    // Verify OutcomeRecorded payload
    if let PoeEvent::OutcomeRecorded {
        outcome, attempts, ..
    } = &events[3].event
    {
        assert!(
            matches!(outcome, PoeOutcomeKind::Success),
            "OutcomeRecorded should report Success"
        );
        assert_eq!(*attempts, 1, "Should succeed on first attempt");
    } else {
        panic!("Expected OutcomeRecorded, got {:?}", events[3].event);
    }
}

/// Budget exhaustion path: worker never satisfies the constraint.
///
/// With max_attempts=2, expect:
///   ManifestCreated, (OperationAttempted + ValidationCompleted) * 2, OutcomeRecorded
///
/// Note: with stuck_window=3 (default) and max_attempts=2, the manager will exhaust
/// the budget before stuck detection fires.
#[tokio::test]
async fn test_poe_event_loop_budget_exhausted() {
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();

    let worker = AlwaysFailingWorker;
    let validator = make_validator();

    // Use stuck_window > max_attempts so budget exhaustion wins over stuck detection
    let config = PoeConfig::new(10, 100_000);
    let manager = PoeManager::new(worker, validator, config).with_event_bus(bus.clone());

    let manifest = SuccessManifest::new("event-test-2", "Impossible task")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("/nonexistent/impossible/file.txt"),
        })
        .with_max_attempts(2);
    let task = PoeTask::new(manifest, "Do the impossible");

    let outcome = manager.execute(task).await.unwrap();
    assert!(
        matches!(outcome, PoeOutcome::BudgetExhausted { .. }),
        "Expected BudgetExhausted, got {:?}",
        outcome
    );

    // Collect events
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(envelope)) => events.push(envelope),
            _ => break,
        }
    }

    // ManifestCreated + 2*(OperationAttempted + ValidationCompleted) + OutcomeRecorded = 6
    assert!(
        events.len() >= 6,
        "Should have at least 6 events for 2-attempt failure, got {} events: {:?}",
        events.len(),
        events.iter().map(|e| e.event_type_tag()).collect::<Vec<_>>()
    );

    // First event: ManifestCreated
    assert_eq!(events[0].event_type_tag(), "ManifestCreated");

    // Last event: OutcomeRecorded with BudgetExhausted
    let last = events.last().unwrap();
    assert_eq!(last.event_type_tag(), "OutcomeRecorded");
    if let PoeEvent::OutcomeRecorded { outcome, .. } = &last.event {
        assert!(
            matches!(outcome, PoeOutcomeKind::BudgetExhausted),
            "Expected BudgetExhausted outcome kind, got {:?}",
            outcome
        );
    } else {
        panic!("Expected OutcomeRecorded, got {:?}", last.event);
    }
}

/// Verify that event sequence numbers are monotonically increasing.
#[tokio::test]
async fn test_poe_events_have_monotonic_seq() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();

    let worker = FileCreatingWorker {
        workspace: tmp.path().to_path_buf(),
    };
    let validator = make_validator();
    let config = PoeConfig::default();
    let manager = PoeManager::new(worker, validator, config).with_event_bus(bus.clone());

    let manifest = SuccessManifest::new("seq-test", "Seq test")
        .with_hard_constraint(ValidationRule::FileExists {
            path: tmp.path().join("test_output.txt"),
        });
    let task = PoeTask::new(manifest, "Create test file");
    manager.execute(task).await.unwrap();

    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(envelope)) => events.push(envelope),
            _ => break,
        }
    }

    assert!(!events.is_empty(), "Should have received events");

    // Verify sequences are strictly monotonically increasing
    for i in 1..events.len() {
        assert!(
            events[i].seq > events[i - 1].seq,
            "Event seq should be monotonically increasing: seq[{}]={} <= seq[{}]={}",
            i,
            events[i].seq,
            i - 1,
            events[i - 1].seq,
        );
    }
}

/// Verify that all events carry the correct task_id.
#[tokio::test]
async fn test_poe_events_carry_correct_task_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();

    let worker = FileCreatingWorker {
        workspace: tmp.path().to_path_buf(),
    };
    let validator = make_validator();
    let config = PoeConfig::default();
    let manager = PoeManager::new(worker, validator, config).with_event_bus(bus.clone());

    let manifest = SuccessManifest::new("task-id-test", "Task ID verification")
        .with_hard_constraint(ValidationRule::FileExists {
            path: tmp.path().join("test_output.txt"),
        });
    let task = PoeTask::new(manifest, "Create test file");
    manager.execute(task).await.unwrap();

    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(envelope)) => events.push(envelope),
            _ => break,
        }
    }

    // Every envelope.task_id should match the task
    for envelope in &events {
        assert_eq!(
            envelope.task_id, "task-id-test",
            "Event {} should carry task_id 'task-id-test', got '{}'",
            envelope.event_type_tag(),
            envelope.task_id
        );
    }

    // Every event payload that has task_id should also match
    for envelope in &events {
        if let Some(inner_task_id) = envelope.event.task_id() {
            assert_eq!(
                inner_task_id, "task-id-test",
                "Inner task_id mismatch in {}",
                envelope.event_type_tag()
            );
        }
    }
}

/// Verify that ValidationCompleted events report correct hard constraint counts.
#[tokio::test]
async fn test_poe_validation_event_reports_constraint_counts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bus = Arc::new(PoeEventBus::default());
    let mut rx = bus.subscribe();

    let worker = FileCreatingWorker {
        workspace: tmp.path().to_path_buf(),
    };
    let validator = make_validator();
    let config = PoeConfig::default();
    let manager = PoeManager::new(worker, validator, config).with_event_bus(bus.clone());

    // Manifest with 1 hard constraint that will pass
    let manifest = SuccessManifest::new("count-test", "Constraint count test")
        .with_hard_constraint(ValidationRule::FileExists {
            path: tmp.path().join("test_output.txt"),
        });
    let task = PoeTask::new(manifest, "Create test file");
    manager.execute(task).await.unwrap();

    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(tokio::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(envelope)) => events.push(envelope),
            _ => break,
        }
    }

    // Find the ValidationCompleted event
    let validation_event = events
        .iter()
        .find(|e| e.event_type_tag() == "ValidationCompleted")
        .expect("Should have a ValidationCompleted event");

    if let PoeEvent::ValidationCompleted {
        passed,
        hard_passed,
        hard_total,
        distance_score,
        ..
    } = &validation_event.event
    {
        assert!(passed, "Validation should have passed");
        assert_eq!(*hard_passed, 1, "1 hard constraint should have passed");
        assert_eq!(*hard_total, 1, "Total hard constraints should be 1");
        assert!(
            *distance_score <= 0.01,
            "Distance should be near 0 for success, got {}",
            distance_score
        );
    } else {
        panic!(
            "Expected ValidationCompleted, got {:?}",
            validation_event.event
        );
    }

    // Also verify ManifestCreated reports constraint counts
    let manifest_event = events
        .iter()
        .find(|e| e.event_type_tag() == "ManifestCreated")
        .expect("Should have a ManifestCreated event");

    if let PoeEvent::ManifestCreated {
        hard_constraints_count,
        soft_metrics_count,
        ..
    } = &manifest_event.event
    {
        assert_eq!(*hard_constraints_count, 1);
        assert_eq!(*soft_metrics_count, 0);
    } else {
        panic!(
            "Expected ManifestCreated, got {:?}",
            manifest_event.event
        );
    }
}
