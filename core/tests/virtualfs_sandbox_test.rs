//! Integration tests for VirtualFs sandbox execution mode
//!
//! Tests the lightweight filesystem isolation provided by VirtualFs:
//! - Environment variable isolation (HOME, TMPDIR, PWD)
//! - Temporary directory creation and cleanup
//! - File write isolation
//! - Security: dangerous env vars removed

use aethecore::tools::markdown_skill::{load_skills_from_dir, SandboxMode};
use aethecore::tools::AetherToolServer;
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper to create a test skill with VirtualFs sandbox
fn create_virtualfs_skill(temp_dir: &TempDir, skill_name: &str, cli_command: &str) -> std::path::PathBuf {
    let skill_dir = temp_dir.path().join(skill_name);
    fs::create_dir(&skill_dir).unwrap();

    let skill_md = skill_dir.join("SKILL.md");
    fs::write(
        &skill_md,
        format!(
            r#"---
name: {skill_name}
description: Test skill for VirtualFs sandbox
metadata:
  requires:
    bins: ["{cli_command}"]
  aether:
    security:
      sandbox: virtualfs
      confirmation: never
      network: internet
---

# {skill_name}

Test skill content.
"#
        ),
    )
    .unwrap();

    skill_dir
}

#[tokio::test]
async fn test_virtualfs_basic_execution() {
    let temp_dir = TempDir::new().unwrap();
    let skill_dir = create_virtualfs_skill(&temp_dir, "echo-test", "echo");

    // Load skill
    let tools = load_skills_from_dir(&skill_dir).await;
    assert_eq!(tools.len(), 1);

    let tool = &tools[0];

    // Verify sandbox mode
    assert!(matches!(
        tool.spec.metadata.aether.as_ref().unwrap().security.sandbox,
        SandboxMode::VirtualFs
    ));

    // Execute with simple args
    let result = tool
        .call(json!({
            "args": ["Hello", "VirtualFs"]
        }))
        .await;

    assert!(result.is_ok(), "VirtualFs execution should succeed");
    let output = result.unwrap();

    assert!(
        output.stdout.contains("Hello VirtualFs"),
        "Output should contain echoed text"
    );
}

#[tokio::test]
async fn test_virtualfs_file_write_isolation() {
    let temp_dir = TempDir::new().unwrap();

    // Create a skill that writes to a file
    let skill_dir = temp_dir.path().join("write-test");
    fs::create_dir(&skill_dir).unwrap();

    let skill_md = skill_dir.join("SKILL.md");
    fs::write(
        &skill_md,
        r#"---
name: write-test
description: Test file writing in VirtualFs
metadata:
  requires:
    bins: ["sh"]
  aether:
    security:
      sandbox: virtualfs
      confirmation: never
---

# Write Test

Test file writing.
"#,
    )
    .unwrap();

    // Load skill
    let tools = load_skills_from_dir(&skill_dir).await;
    let tool = &tools[0];

    // Execute command that writes to a file in PWD
    let result = tool
        .call(json!({
            "args": ["-c", "echo 'test content' > testfile.txt && cat testfile.txt"]
        }))
        .await;

    assert!(result.is_ok(), "File write should succeed in sandbox");

    let output = result.unwrap();

    // Verify the file was written and read successfully
    assert!(
        output.stdout.contains("test content"),
        "Should be able to write and read file in sandbox"
    );

    // Verify the file is NOT in the real skill directory
    let real_file_path = skill_dir.join("testfile.txt");
    assert!(
        !real_file_path.exists(),
        "File should NOT exist in real filesystem"
    );
}

#[tokio::test]
async fn test_virtualfs_env_variables() {
    let temp_dir = TempDir::new().unwrap();

    // Create a skill that prints environment variables
    let skill_dir = temp_dir.path().join("env-test");
    fs::create_dir(&skill_dir).unwrap();

    let skill_md = skill_dir.join("SKILL.md");
    fs::write(
        &skill_md,
        r#"---
name: env-test
description: Test environment variables in VirtualFs
metadata:
  requires:
    bins: ["sh"]
  aether:
    security:
      sandbox: virtualfs
      confirmation: never
---

# Env Test

Test environment variables.
"#,
    )
    .unwrap();

    // Load skill
    let tools = load_skills_from_dir(&skill_dir).await;
    let tool = &tools[0];

    // Execute command that prints environment variables
    let result = tool
        .call(json!({
            "args": ["-c", "echo HOME=$HOME; echo TMPDIR=$TMPDIR; echo PWD=$PWD"]
        }))
        .await
        .unwrap();

    // Verify environment variables are sandboxed
    assert!(
        result.stdout.contains("HOME=") && result.stdout.contains("virtualfs"),
        "HOME should be in sandbox"
    );
    assert!(
        result.stdout.contains("TMPDIR=") && result.stdout.contains("virtualfs"),
        "TMPDIR should be in sandbox"
    );
    assert!(
        result.stdout.contains("PWD=") && result.stdout.contains("virtualfs"),
        "PWD should be in sandbox"
    );
}

#[tokio::test]
async fn test_virtualfs_tmp_directory_usage() {
    let temp_dir = TempDir::new().unwrap();
    let skill_dir = temp_dir.path().join("tmp-test");
    fs::create_dir(&skill_dir).unwrap();

    let skill_md = skill_dir.join("SKILL.md");
    fs::write(
        &skill_md,
        r#"---
name: tmp-test
description: Test tmp directory in VirtualFs
metadata:
  requires:
    bins: ["sh"]
  aether:
    security:
      sandbox: virtualfs
      confirmation: never
---

# Tmp Test
"#,
    )
    .unwrap();

    // Load skill
    let tools = load_skills_from_dir(&skill_dir).await;
    let tool = &tools[0];

    // Execute command that uses $TMPDIR
    let result = tool
        .call(json!({
            "args": ["-c", "echo 'temp data' > $TMPDIR/temp.txt && cat $TMPDIR/temp.txt"]
        }))
        .await
        .unwrap();

    assert!(
        result.stdout.contains("temp data"),
        "Should be able to write to TMPDIR"
    );
}

#[tokio::test]
async fn test_virtualfs_cleanup() {
    let temp_dir = TempDir::new().unwrap();
    let skill_dir = create_virtualfs_skill(&temp_dir, "cleanup-test", "echo");

    // Load skill
    let tools = load_skills_from_dir(&skill_dir).await;
    let tool = &tools[0];

    // Get system temp directory
    let system_temp = std::env::temp_dir();

    // Count sandbox directories before
    let count_before = count_virtualfs_sandboxes(&system_temp);

    // Execute multiple times
    for _ in 0..3 {
        let _ = tool
            .call(json!({
                "args": ["test"]
            }))
            .await;
    }

    // Wait a bit for cleanup
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Count sandbox directories after
    let count_after = count_virtualfs_sandboxes(&system_temp);

    // Sandboxes should be cleaned up (count should not increase by 3)
    // Allow for some variance due to other concurrent tests
    let diff = if count_after >= count_before {
        count_after - count_before
    } else {
        0
    };
    assert!(
        diff < 3,
        "Sandbox directories should be cleaned up (before: {}, after: {})",
        count_before,
        count_after
    );
}

#[tokio::test]
async fn test_virtualfs_in_tool_server() {
    let temp_dir = TempDir::new().unwrap();
    let skill_dir = create_virtualfs_skill(&temp_dir, "server-test", "echo");

    // Load skill into ToolServer
    let tool_server = Arc::new(AetherToolServer::new());
    let tools = load_skills_from_dir(&skill_dir).await;

    for tool in tools {
        tool_server.add_tool(tool).await;
    }

    // Call via ToolServer
    let result = tool_server
        .call("server-test", json!({"args": ["ServerTest"]}))
        .await
        .unwrap();

    let stdout = result.get("stdout").unwrap().as_str().unwrap();
    assert!(
        stdout.contains("ServerTest"),
        "VirtualFs should work via ToolServer"
    );
}

#[tokio::test]
async fn test_virtualfs_error_handling() {
    let temp_dir = TempDir::new().unwrap();

    // Create a skill with non-existent command
    let skill_dir = temp_dir.path().join("error-test");
    fs::create_dir(&skill_dir).unwrap();

    let skill_md = skill_dir.join("SKILL.md");
    fs::write(
        &skill_md,
        r#"---
name: error-test
description: Test error handling in VirtualFs
metadata:
  requires:
    bins: ["nonexistent-command-12345"]
  aether:
    security:
      sandbox: virtualfs
      confirmation: never
---

# Error Test
"#,
    )
    .unwrap();

    // Load skill
    let tools = load_skills_from_dir(&skill_dir).await;
    let tool = &tools[0];

    // Execute with non-existent command
    let result = tool.call(json!({"args": []})).await;

    assert!(
        result.is_err(),
        "Should fail with non-existent command"
    );
}

/// Helper to count VirtualFs sandbox directories in system temp
fn count_virtualfs_sandboxes(temp_dir: &std::path::Path) -> usize {
    std::fs::read_dir(temp_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("aether-virtualfs-")
                })
                .count()
        })
        .unwrap_or(0)
}
