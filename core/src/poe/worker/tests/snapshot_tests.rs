//! Tests for StateSnapshot git-based capture/restore.

use crate::poe::worker::StateSnapshot;
use tempfile::TempDir;

#[test]
fn test_snapshot_new_has_no_stash() {
    let snapshot = StateSnapshot::new("/tmp/test".into());
    assert!(snapshot.stash_hash.is_none());
    assert_eq!(snapshot.file_count(), 0);
}

#[test]
fn test_snapshot_with_files_has_no_stash() {
    let snapshot = StateSnapshot::with_files(
        "/tmp/test".into(),
        vec![("file.rs".into(), "abc123".into())],
    );
    assert!(snapshot.stash_hash.is_none());
    assert_eq!(snapshot.file_count(), 1);
}

#[tokio::test]
async fn test_has_git_in_non_git_dir() {
    let tmp = TempDir::new().unwrap();
    assert!(!StateSnapshot::has_git(tmp.path()).await);
}

#[tokio::test]
async fn test_capture_non_git_dir() {
    let tmp = TempDir::new().unwrap();
    let snapshot = StateSnapshot::capture(tmp.path()).await.unwrap();
    assert!(snapshot.stash_hash.is_none());
}

#[tokio::test]
async fn test_capture_git_dir_no_changes() {
    let tmp = TempDir::new().unwrap();
    // Initialize git repo
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    // Configure git user for the test repo
    tokio::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    // Create initial commit
    std::fs::write(tmp.path().join("README.md"), "test").unwrap();
    tokio::process::Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();

    // Capture with no changes
    let snapshot = StateSnapshot::capture(tmp.path()).await.unwrap();
    assert!(snapshot.stash_hash.is_none(), "No changes = no stash hash");
}

#[tokio::test]
async fn test_capture_and_restore() {
    let tmp = TempDir::new().unwrap();
    // Init git
    tokio::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();

    // Create initial state
    std::fs::write(tmp.path().join("file.txt"), "original").unwrap();
    tokio::process::Command::new("git")
        .args(["add", "."])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(tmp.path())
        .output()
        .await
        .unwrap();

    // Make changes
    std::fs::write(tmp.path().join("file.txt"), "modified").unwrap();

    // Capture
    let snapshot = StateSnapshot::capture(tmp.path()).await.unwrap();
    assert!(
        snapshot.stash_hash.is_some(),
        "Modified file should create stash"
    );

    // Make more changes (simulate failed attempt)
    std::fs::write(tmp.path().join("file.txt"), "broken").unwrap();

    // Restore
    snapshot.restore().await.unwrap();

    // Verify restored state
    let content = std::fs::read_to_string(tmp.path().join("file.txt")).unwrap();
    assert_eq!(content, "modified", "Should restore to captured state");
}

#[tokio::test]
async fn test_restore_noop_without_stash() {
    let snapshot = StateSnapshot::new("/tmp/test".into());
    // Should not panic or error
    snapshot.restore().await.unwrap();
}
