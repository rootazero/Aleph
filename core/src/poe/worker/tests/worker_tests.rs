//! Tests for Worker trait and implementations.

use std::path::PathBuf;

use crate::poe::worker::callback::PoeLoopCallback;
use crate::poe::worker::{StateSnapshot, Worker};
use crate::poe::types::ChangeType;

use super::mock_worker::MockWorker;

#[test]
fn test_state_snapshot_creation() {
    let snapshot = StateSnapshot::new(PathBuf::from("/workspace"));

    assert_eq!(snapshot.workspace, PathBuf::from("/workspace"));
    assert!(snapshot.file_hashes.is_empty());
    assert_eq!(snapshot.file_count(), 0);
}

#[test]
fn test_state_snapshot_with_files() {
    let files = vec![
        (PathBuf::from("foo.rs"), "abc123".to_string()),
        (PathBuf::from("bar.rs"), "def456".to_string()),
    ];

    let snapshot = StateSnapshot::with_files(PathBuf::from("/workspace"), files);

    assert_eq!(snapshot.file_count(), 2);
    assert!(snapshot.contains_file(&PathBuf::from("foo.rs")));
    assert!(!snapshot.contains_file(&PathBuf::from("baz.rs")));
    assert_eq!(
        snapshot.get_file_hash(&PathBuf::from("foo.rs")),
        Some("abc123")
    );
    assert_eq!(
        snapshot.get_file_hash(&PathBuf::from("bar.rs")),
        Some("def456")
    );
    assert_eq!(snapshot.get_file_hash(&PathBuf::from("baz.rs")), None);
}

#[tokio::test]
async fn test_mock_worker_success() {
    let worker = MockWorker::new().with_tokens(200);

    let output = worker.execute("test", None).await.unwrap();

    assert!(matches!(
        output.final_state,
        crate::poe::types::WorkerState::Completed { .. }
    ));
    assert_eq!(output.tokens_consumed, 200);
    assert_eq!(worker.execution_count(), 1);
}

#[tokio::test]
async fn test_mock_worker_failure() {
    let worker = MockWorker::failing();

    let output = worker.execute("test", None).await.unwrap();

    assert!(matches!(
        output.final_state,
        crate::poe::types::WorkerState::Failed { .. }
    ));
    assert_eq!(worker.execution_count(), 1);
}

#[tokio::test]
async fn test_mock_worker_multiple_executions() {
    let worker = MockWorker::new();

    worker.execute("first", None).await.unwrap();
    worker.execute("second", None).await.unwrap();
    worker.execute("third", None).await.unwrap();

    assert_eq!(worker.execution_count(), 3);
}

#[tokio::test]
async fn test_mock_worker_abort() {
    let worker = MockWorker::new();

    // Abort should always succeed
    assert!(worker.abort().await.is_ok());
}

#[tokio::test]
async fn test_mock_worker_snapshot_restore() {
    let worker = MockWorker::new();

    let snapshot = worker.snapshot().await.unwrap();
    assert!(worker.restore(&snapshot).await.is_ok());
}

#[test]
fn test_poe_callback_extract_file_path() {
    let args = serde_json::json!({
        "path": "/tmp/test.txt"
    });
    assert_eq!(
        PoeLoopCallback::extract_file_path(&args),
        Some(PathBuf::from("/tmp/test.txt"))
    );

    let args2 = serde_json::json!({
        "file_path": "/tmp/other.rs"
    });
    assert_eq!(
        PoeLoopCallback::extract_file_path(&args2),
        Some(PathBuf::from("/tmp/other.rs"))
    );

    let args3 = serde_json::json!({
        "unrelated": "value"
    });
    assert_eq!(PoeLoopCallback::extract_file_path(&args3), None);
}

#[test]
fn test_poe_callback_change_type_from_tool() {
    let args = serde_json::json!({});

    assert!(matches!(
        PoeLoopCallback::change_type_from_tool("write_file", &args),
        ChangeType::Created
    ));
    assert!(matches!(
        PoeLoopCallback::change_type_from_tool("edit_file", &args),
        ChangeType::Modified
    ));
    assert!(matches!(
        PoeLoopCallback::change_type_from_tool("delete_file", &args),
        ChangeType::Deleted
    ));

    // With operation field
    let args_delete = serde_json::json!({
        "operation": "delete"
    });
    assert!(matches!(
        PoeLoopCallback::change_type_from_tool("file_ops", &args_delete),
        ChangeType::Deleted
    ));
}

#[test]
fn test_poe_callback_compute_hash() {
    let hash = PoeLoopCallback::compute_hash("hello world");
    // SHA-256 of "hello world"
    assert_eq!(
        hash,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn test_worker_executor_creation_with_gate() {
    // Compilation test: verifies ExecSecurityGate wiring compiles correctly
    use crate::exec::ExecApprovalManager;
    use crate::executor::{ExecSecurityGate, SingleStepExecutor, BuiltinToolRegistry};
    use crate::sync_primitives::Arc;

    let tool_registry = Arc::new(BuiltinToolRegistry::new());
    let approval_manager = Arc::new(ExecApprovalManager::new());
    let gate = Arc::new(ExecSecurityGate::new(approval_manager, None));
    let _executor = SingleStepExecutor::new(tool_registry)
        .with_exec_security_gate(gate);
    // If this compiles, the wiring is correct
}
