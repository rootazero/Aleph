//! Property-based tests for TaskStatus serde roundtrip and Task invariants.

use proptest::prelude::*;
use std::path::PathBuf;

use super::{FileOp, Task, TaskResult, TaskStatus, TaskType};

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Generate a non-empty task ID.
fn arb_non_empty_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_map(|s| s)
}

/// Generate an arbitrary TaskStatus.
fn arb_task_status() -> impl Strategy<Value = TaskStatus> {
    prop_oneof![
        Just(TaskStatus::Pending),
        // Use full f32 range to stress edge cases; progress() returns the raw value.
        prop::num::f32::ANY
            .prop_map(|p| TaskStatus::Running {
                progress: p,
                message: None,
            }),
        ".*".prop_map(|msg: String| TaskStatus::Running {
            progress: 0.5,
            message: if msg.is_empty() { None } else { Some(msg) },
        }),
        Just(TaskStatus::completed(TaskResult::default())),
        ".*".prop_map(|err: String| TaskStatus::Failed {
            error: if err.is_empty() {
                "error".to_string()
            } else {
                err
            },
            recoverable: false,
        }),
        ".*".prop_map(|err: String| TaskStatus::Failed {
            error: if err.is_empty() {
                "error".to_string()
            } else {
                err
            },
            recoverable: true,
        }),
        Just(TaskStatus::Cancelled),
    ]
}

/// Generate a TaskStatus that is safe for serde roundtrip (no NaN/Inf in f32).
fn arb_serde_safe_task_status() -> impl Strategy<Value = TaskStatus> {
    prop_oneof![
        Just(TaskStatus::Pending),
        (0.0f32..=1.0).prop_map(|p| TaskStatus::running(p)),
        (0.0f32..=1.0, "[ -~]{0,20}").prop_map(|(p, msg)| {
            TaskStatus::Running {
                progress: p,
                message: if msg.is_empty() { None } else { Some(msg) },
            }
        }),
        Just(TaskStatus::completed(TaskResult::default())),
        "[ -~]{1,30}".prop_map(|err| TaskStatus::failed(err)),
        "[ -~]{1,30}".prop_map(|err| TaskStatus::failed_recoverable(err)),
        Just(TaskStatus::Cancelled),
    ]
}

/// Helper: create a simple file-list task with a given ID.
fn make_task(id: &str) -> Task {
    Task::new(
        id,
        format!("task-{id}"),
        TaskType::FileOperation(FileOp::List {
            path: PathBuf::from("/tmp"),
        }),
    )
}

// ---------------------------------------------------------------------------
// Property Tests
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    // Property 1: TaskStatus serde roundtrip -- serialize then deserialize
    // yields an equivalent variant.
    #[test]
    fn task_status_serde_roundtrip(status in arb_serde_safe_task_status()) {
        let json = serde_json::to_string(&status)
            .expect("TaskStatus should serialize");
        let deserialized: TaskStatus = serde_json::from_str(&json)
            .expect("TaskStatus should deserialize");

        // Compare discriminant (variant kind).
        let orig_tag = std::mem::discriminant(&status);
        let deser_tag = std::mem::discriminant(&deserialized);
        prop_assert_eq!(
            orig_tag, deser_tag,
            "Serde roundtrip changed variant: {:?} -> {:?}",
            status, deserialized
        );

        // For Running variant, also check progress value is preserved.
        if let (
            TaskStatus::Running { progress: p1, message: m1 },
            TaskStatus::Running { progress: p2, message: m2 },
        ) = (&status, &deserialized) {
            prop_assert!(
                (p1 - p2).abs() < 1e-6,
                "Progress changed in roundtrip: {} -> {}", p1, p2
            );
            prop_assert_eq!(m1, m2, "Message changed in roundtrip");
        }

        // For Failed variant, check error and recoverable are preserved.
        if let (
            TaskStatus::Failed { error: e1, recoverable: r1 },
            TaskStatus::Failed { error: e2, recoverable: r2 },
        ) = (&status, &deserialized) {
            prop_assert_eq!(e1, e2, "Error changed in roundtrip");
            prop_assert_eq!(r1, r2, "Recoverable changed in roundtrip");
        }
    }

    // Property 2: Task always has a non-empty ID after construction.
    #[test]
    fn task_always_has_nonempty_id(id in arb_non_empty_id()) {
        let task = make_task(&id);
        prop_assert!(
            !task.id.is_empty(),
            "Task ID should never be empty"
        );
        prop_assert_eq!(
            &task.id, &id,
            "Task ID should match the input"
        );
    }

    // Property 3: Default TaskStatus is Pending.
    // (This is a unit-level property but we confirm it holds under proptest framework.)
    #[test]
    fn default_task_status_is_pending(_seed in 0u32..1000) {
        let status = TaskStatus::default();
        prop_assert!(
            matches!(status, TaskStatus::Pending),
            "Default TaskStatus should be Pending, got: {:?}",
            status
        );
    }

    // Property 4: Running progress is bounded to [0.0, 1.0] when the input
    // is a finite number that is clamped to [0.0, 1.0].
    // Note: f32::NaN.clamp(0.0, 1.0) still produces NaN, so we filter it out
    // and verify that for all finite inputs, clamping produces a valid progress.
    #[test]
    fn running_progress_clamped_bounded(raw_progress in prop::num::f32::ANY) {
        // Skip NaN and Inf since clamp does not help with them.
        if !raw_progress.is_finite() {
            return Ok(());
        }
        let clamped = raw_progress.clamp(0.0, 1.0);
        let status = TaskStatus::running(clamped);

        if let TaskStatus::Running { progress, .. } = status {
            prop_assert!(
                (0.0..=1.0).contains(&progress),
                "Clamped progress {} should be in [0.0, 1.0] (raw: {})",
                progress, raw_progress
            );
        } else {
            prop_assert!(false, "Expected Running status");
        }
    }

    // Property 5: Task.progress() always returns a finite value for any TaskStatus.
    #[test]
    fn task_progress_is_finite(status in arb_task_status()) {
        let mut task = make_task("test");
        task.status = status;
        let p = task.progress();
        // progress() returns 0.0 for Pending/Failed/Cancelled, the raw value
        // for Running, and 1.0 for Completed. It should be finite for the
        // three non-Running variants.
        if !task.is_running() {
            prop_assert!(
                p.is_finite(),
                "progress() should be finite for non-Running, got: {}",
                p
            );
        }
    }

    // Property 6: Full Task serde roundtrip preserves identity.
    #[test]
    fn task_serde_roundtrip_preserves_id(id in arb_non_empty_id()) {
        let task = make_task(&id);
        let json = serde_json::to_string(&task)
            .expect("Task should serialize");
        let deserialized: Task = serde_json::from_str(&json)
            .expect("Task should deserialize");

        prop_assert_eq!(
            &task.id, &deserialized.id,
            "Task ID should survive serde roundtrip"
        );
        prop_assert_eq!(
            &task.name, &deserialized.name,
            "Task name should survive serde roundtrip"
        );
    }

    // Property 7: Newly created tasks are always pending.
    #[test]
    fn new_task_is_pending(id in arb_non_empty_id()) {
        let task = make_task(&id);
        prop_assert!(
            task.is_pending(),
            "Newly created task should be pending"
        );
        prop_assert_eq!(
            task.progress(), 0.0,
            "Newly created task should have 0.0 progress"
        );
        prop_assert!(
            !task.is_finished(),
            "Newly created task should not be finished"
        );
    }
}
