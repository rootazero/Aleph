//! Step definitions for security features (VirtualFs sandbox)

use crate::world::{AlephWorld, SecurityContext, SkillExecutionResult};
use alephcore::tools::markdown_skill::{load_skills_from_dir, SandboxMode, MarkdownToolOutput};
use alephcore::tools::AlephToolServer;
use alephcore::AlephError;
use cucumber::{given, then, when};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

// ═══ Helper Functions ═══

/// Create a VirtualFs skill in the given directory
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
  aleph:
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

/// Count VirtualFs sandbox directories in system temp
fn count_virtualfs_sandboxes(temp_dir: &std::path::Path) -> usize {
    std::fs::read_dir(temp_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("aleph-virtualfs-")
                })
                .count()
        })
        .unwrap_or(0)
}

// ═══ Setup Steps ═══

#[given("a temporary skill directory")]
async fn given_temp_skill_dir(w: &mut AlephWorld) {
    let ctx = w.security.get_or_insert_with(SecurityContext::default);
    ctx.temp_dir = Some(TempDir::new().unwrap());
}

#[given(expr = "a VirtualFs skill named {string} using {string}")]
async fn given_virtualfs_skill(w: &mut AlephWorld, skill_name: String, cli_command: String) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp directory not set");
    let skill_dir = create_virtualfs_skill(temp_dir, &skill_name, &cli_command);
    ctx.skill_dir = Some(skill_dir);
}

#[given("an empty tool server")]
async fn given_empty_tool_server(w: &mut AlephWorld) {
    let ctx = w.security.get_or_insert_with(SecurityContext::default);
    ctx.tool_server = Some(Arc::new(AlephToolServer::new()));
}

// ═══ When Steps ═══

#[when("I load VirtualFs skills from the directory")]
async fn when_load_virtualfs_skills(w: &mut AlephWorld) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let skill_dir = ctx.skill_dir.as_ref().expect("Skill directory not set");
    let tools = load_skills_from_dir(skill_dir).await;
    ctx.loaded_tools = tools;
}

#[when(expr = "I call the skill with args {word}")]
async fn when_call_skill_args(w: &mut AlephWorld, args_json: String) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let tool = ctx.loaded_tools.first().expect("No tools loaded");

    // Parse args like ["Hello", "VirtualFs"]
    let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();

    let result: Result<MarkdownToolOutput, AlephError> = tool.call(json!({ "args": args })).await;

    ctx.execution_result = Some(match result {
        Ok(output) => Ok(SkillExecutionResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        }),
        Err(e) => Err(e.to_string()),
    });
}

#[when(expr = "I call the skill with shell command {string}")]
async fn when_call_skill_shell(w: &mut AlephWorld, command: String) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let tool = ctx.loaded_tools.first().expect("No tools loaded");

    let result: Result<MarkdownToolOutput, AlephError> = tool.call(json!({ "args": ["-c", command] })).await;

    ctx.execution_result = Some(match result {
        Ok(output) => Ok(SkillExecutionResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        }),
        Err(e) => Err(e.to_string()),
    });
}

#[when("I count sandbox directories before execution")]
async fn when_count_sandboxes_before(w: &mut AlephWorld) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let system_temp = std::env::temp_dir();
    ctx.sandbox_count_before = count_virtualfs_sandboxes(&system_temp);
}

#[when("I execute the skill multiple times")]
async fn when_execute_multiple_times(w: &mut AlephWorld) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let tool = ctx.loaded_tools.first().expect("No tools loaded");

    for _ in 0..3 {
        let _ = tool.call(json!({ "args": ["test"] })).await;
    }
}

#[when("I wait briefly for cleanup")]
async fn when_wait_cleanup(w: &mut AlephWorld) {
    let _ = w;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
}

#[when("I count sandbox directories after execution")]
async fn when_count_sandboxes_after(w: &mut AlephWorld) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let system_temp = std::env::temp_dir();
    ctx.sandbox_count_after = count_virtualfs_sandboxes(&system_temp);
}

#[when("I load VirtualFs skills into the tool server")]
async fn when_load_virtualfs_skills_to_server(w: &mut AlephWorld) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let skill_dir = ctx.skill_dir.as_ref().expect("Skill directory not set");
    let tools = load_skills_from_dir(skill_dir).await;

    let server = ctx.tool_server.as_ref().expect("Tool server not initialized");
    for tool in tools {
        server.add_tool(tool).await;
    }
}

#[when(expr = "I call {string} via tool server with args {word}")]
async fn when_call_via_server(w: &mut AlephWorld, tool_name: String, args_json: String) {
    let ctx = w.security.as_mut().expect("Security context not initialized");
    let server = ctx.tool_server.as_ref().expect("Tool server not initialized");

    let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
    let result = server.call(&tool_name, json!({ "args": args })).await;

    ctx.tool_server_result = result.ok();
}

// ═══ Then Steps ═══

#[then("the skill should be loaded successfully")]
async fn then_skill_loaded(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    assert!(!ctx.loaded_tools.is_empty(), "At least one tool should be loaded");
}

#[then("the sandbox mode should be VirtualFs")]
async fn then_sandbox_virtualfs(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let tool = ctx.loaded_tools.first().expect("No tools loaded");

    assert!(
        matches!(
            tool.spec.metadata.aleph.as_ref().unwrap().security.sandbox,
            SandboxMode::VirtualFs
        ),
        "Sandbox mode should be VirtualFs"
    );
}

#[then("the skill execution should succeed")]
async fn then_skill_execution_success(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    assert!(result.is_ok(), "Execution should succeed: {:?}", result);
}

#[then(expr = "the stdout should contain {string}")]
async fn then_stdout_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    let output = result.as_ref().expect("Execution failed");
    assert!(
        output.stdout.contains(&expected),
        "stdout '{}' should contain '{}'",
        output.stdout, expected
    );
}

#[then(expr = "{string} should NOT exist in the real skill directory")]
async fn then_file_not_exists(w: &mut AlephWorld, filename: String) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let skill_dir = ctx.skill_dir.as_ref().expect("Skill directory not set");
    let file_path = skill_dir.join(&filename);
    assert!(
        !file_path.exists(),
        "File should NOT exist in real filesystem: {:?}",
        file_path
    );
}

#[then("the stdout should indicate HOME is sandboxed")]
async fn then_home_sandboxed(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    let output = result.as_ref().expect("Execution failed");
    assert!(
        output.stdout.contains("HOME=") && output.stdout.contains("virtualfs"),
        "HOME should be in sandbox: {}",
        output.stdout
    );
}

#[then("the stdout should indicate TMPDIR is sandboxed")]
async fn then_tmpdir_sandboxed(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    let output = result.as_ref().expect("Execution failed");
    assert!(
        output.stdout.contains("TMPDIR=") && output.stdout.contains("virtualfs"),
        "TMPDIR should be in sandbox: {}",
        output.stdout
    );
}

#[then("the stdout should indicate PWD is sandboxed")]
async fn then_pwd_sandboxed(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let result = ctx.execution_result.as_ref().expect("No execution result");
    let output = result.as_ref().expect("Execution failed");
    assert!(
        output.stdout.contains("PWD=") && output.stdout.contains("virtualfs"),
        "PWD should be in sandbox: {}",
        output.stdout
    );
}

#[then("sandbox directories should not accumulate")]
async fn then_sandboxes_not_accumulate(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let diff = if ctx.sandbox_count_after >= ctx.sandbox_count_before {
        ctx.sandbox_count_after - ctx.sandbox_count_before
    } else {
        0
    };
    assert!(
        diff < 3,
        "Sandbox directories should be cleaned up (before: {}, after: {})",
        ctx.sandbox_count_before,
        ctx.sandbox_count_after
    );
}

#[then("the tool server call should succeed")]
async fn then_server_call_success(w: &mut AlephWorld) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    assert!(
        ctx.tool_server_result.is_some(),
        "Tool server call should succeed"
    );
}

#[then(expr = "the result stdout should contain {string}")]
async fn then_result_stdout_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.security.as_ref().expect("Security context not initialized");
    let result = ctx.tool_server_result.as_ref().expect("No tool server result");
    let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        stdout.contains(&expected),
        "Result stdout '{}' should contain '{}'",
        stdout, expected
    );
}
