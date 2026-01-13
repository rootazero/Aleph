//! Integration tests for Native Function Calling Tools
//!
//! Tests the AgentTool infrastructure including:
//! - Filesystem operations (read, write, list, delete, search)
//! - Git operations (status, diff, log, branch)
//! - Shell execution (with security constraints)
//! - Tool registry operations
//! - Tool execution flow

use aethecore::tools::{
    create_filesystem_tools, create_git_tools, create_shell_tools, create_system_tools,
    FilesystemConfig, GitConfig, NativeToolRegistry, ShellConfig, ToolCategory,
};
use std::path::PathBuf;
use tempfile::TempDir;

// =============================================================================
// Filesystem Tools Integration Tests
// =============================================================================

#[tokio::test]
async fn test_filesystem_tools_file_read_write() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Create config allowing access to temp directory
    let config = FilesystemConfig::new(vec![temp_path.clone()]);
    let tools = create_filesystem_tools(config);

    // Register tools
    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test file_write
    let test_file = temp_path.join("test.txt");
    let write_args = serde_json::json!({
        "path": test_file.to_str().unwrap(),
        "content": "Hello, World!"
    });

    let write_result = registry
        .execute("file_write", &write_args.to_string())
        .await;
    assert!(write_result.is_ok(), "file_write should succeed");
    let write_result = write_result.unwrap();
    assert!(write_result.is_success(), "file_write should return success");

    // Verify file was created
    assert!(test_file.exists(), "File should exist after write");

    // Test file_read
    let read_args = serde_json::json!({
        "path": test_file.to_str().unwrap()
    });

    let read_result = registry
        .execute("file_read", &read_args.to_string())
        .await;
    assert!(read_result.is_ok(), "file_read should succeed");
    let read_result = read_result.unwrap();
    assert!(read_result.is_success(), "file_read should return success");
    assert!(
        read_result.content.contains("Hello, World!"),
        "file_read should return written content"
    );
}

#[tokio::test]
async fn test_filesystem_tools_file_list() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Create some test files
    std::fs::write(temp_path.join("file1.txt"), "content1").unwrap();
    std::fs::write(temp_path.join("file2.txt"), "content2").unwrap();
    std::fs::create_dir(temp_path.join("subdir")).unwrap();

    let config = FilesystemConfig::new(vec![temp_path.clone()]);
    let tools = create_filesystem_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test file_list
    let list_args = serde_json::json!({
        "path": temp_path.to_str().unwrap()
    });

    let result = registry
        .execute("file_list", &list_args.to_string())
        .await;
    assert!(result.is_ok(), "file_list should succeed");
    let result = result.unwrap();
    assert!(result.is_success(), "file_list should return success");
    assert!(result.content.contains("file1.txt"), "Should list file1.txt");
    assert!(result.content.contains("file2.txt"), "Should list file2.txt");
    assert!(result.content.contains("subdir"), "Should list subdir");
}

#[tokio::test]
async fn test_filesystem_tools_file_search() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Create test files with different extensions
    std::fs::write(temp_path.join("test1.rs"), "rust code").unwrap();
    std::fs::write(temp_path.join("test2.rs"), "more rust").unwrap();
    std::fs::write(temp_path.join("test.txt"), "text file").unwrap();

    let config = FilesystemConfig::new(vec![temp_path.clone()]);
    let tools = create_filesystem_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test file_search with glob pattern
    // Note: uses "base" and "pattern" parameters
    let search_args = serde_json::json!({
        "base": temp_path.to_str().unwrap(),
        "pattern": "*.rs"
    });

    let result = registry
        .execute("file_search", &search_args.to_string())
        .await;
    assert!(result.is_ok(), "file_search should succeed");
    let result = result.unwrap();
    assert!(result.is_success(), "file_search should return success");
    assert!(result.content.contains("test1.rs"), "Should find test1.rs");
    assert!(result.content.contains("test2.rs"), "Should find test2.rs");
    assert!(
        !result.content.contains("test.txt"),
        "Should not find test.txt"
    );
}

#[tokio::test]
async fn test_filesystem_tools_file_delete() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Create a test file
    let test_file = temp_path.join("to_delete.txt");
    std::fs::write(&test_file, "delete me").unwrap();
    assert!(test_file.exists(), "File should exist before delete");

    let config = FilesystemConfig::new(vec![temp_path.clone()]);
    let tools = create_filesystem_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test file_delete
    let delete_args = serde_json::json!({
        "path": test_file.to_str().unwrap()
    });

    let result = registry
        .execute("file_delete", &delete_args.to_string())
        .await;
    assert!(result.is_ok(), "file_delete should succeed");
    let result = result.unwrap();
    assert!(result.is_success(), "file_delete should return success");
    assert!(!test_file.exists(), "File should not exist after delete");
}

#[tokio::test]
async fn test_filesystem_tools_permission_denied() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Create config allowing only temp directory
    let config = FilesystemConfig::new(vec![temp_path]);
    let tools = create_filesystem_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Try to read file outside allowed directory
    let read_args = serde_json::json!({
        "path": "/etc/passwd"
    });

    let result = registry
        .execute("file_read", &read_args.to_string())
        .await;
    // Path validation returns an error (not ToolResult), which is expected
    assert!(
        result.is_err(),
        "Should return error when accessing outside allowed roots"
    );
}

// =============================================================================
// Git Tools Integration Tests
// =============================================================================

#[tokio::test]
async fn test_git_tools_in_repo() {
    // Use current project directory as a git repo
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Create config allowing access to project
    let config = GitConfig::new(vec![project_root.clone()]);
    let tools = create_git_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test git_status
    let status_args = serde_json::json!({
        "path": project_root.to_str().unwrap()
    });

    let result = registry
        .execute("git_status", &status_args.to_string())
        .await;
    assert!(result.is_ok(), "git_status should succeed");
    let result = result.unwrap();
    assert!(result.is_success(), "git_status should return success");
    // Should have some output (either clean or with changes)
    assert!(!result.content.is_empty(), "git_status should have output");

    // Test git_branch
    let branch_args = serde_json::json!({
        "path": project_root.to_str().unwrap()
    });

    let result = registry
        .execute("git_branch", &branch_args.to_string())
        .await;
    assert!(result.is_ok(), "git_branch should succeed");
    let result = result.unwrap();
    assert!(result.is_success(), "git_branch should return success");
    // Should return a branch name
    assert!(!result.content.is_empty(), "git_branch should return branch name");
}

#[tokio::test]
async fn test_git_tools_log() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let config = GitConfig::new(vec![project_root.clone()]);
    let tools = create_git_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test git_log with limit
    let log_args = serde_json::json!({
        "path": project_root.to_str().unwrap(),
        "limit": 5
    });

    let result = registry
        .execute("git_log", &log_args.to_string())
        .await;
    assert!(result.is_ok(), "git_log should succeed");
    let result = result.unwrap();
    assert!(result.is_success(), "git_log should return success");
    // Should have commit information
    assert!(!result.content.is_empty(), "git_log should have output");
}

#[tokio::test]
async fn test_git_tools_not_a_repo() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    // Allow access to temp directory (not a git repo)
    let config = GitConfig::new(vec![temp_path.clone()]);
    let tools = create_git_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test git_status in non-repo directory
    let status_args = serde_json::json!({
        "path": temp_path.to_str().unwrap()
    });

    let result = registry
        .execute("git_status", &status_args.to_string())
        .await;
    assert!(result.is_ok(), "Should return result");
    let result = result.unwrap();
    // Should fail because it's not a git repository
    assert!(
        !result.is_success(),
        "git_status should fail in non-repo directory"
    );
}

// =============================================================================
// Shell Tools Integration Tests
// =============================================================================

#[tokio::test]
async fn test_shell_tools_disabled_by_default() {
    // Default config has shell disabled
    let config = ShellConfig::default();
    let tools = create_shell_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Try to execute a simple command
    // Note: tool name is "shell_exec", not "shell_execute"
    let args = serde_json::json!({
        "command": "echo hello"
    });

    let result = registry
        .execute("shell_exec", &args.to_string())
        .await;
    // When shell is disabled, validation returns an error
    assert!(
        result.is_err(),
        "shell_exec should return error when disabled"
    );
}

#[tokio::test]
async fn test_shell_tools_enabled_with_whitelist() {
    // Create config with shell enabled and whitelist
    let mut config = ShellConfig::default();
    config.enabled = true;
    config.allowed_commands = vec!["echo".to_string(), "pwd".to_string()];

    let tools = create_shell_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test allowed command
    // Note: tool name is "shell_exec"
    let args = serde_json::json!({
        "command": "echo hello"
    });

    let result = registry
        .execute("shell_exec", &args.to_string())
        .await;
    assert!(result.is_ok(), "shell_exec should succeed: {:?}", result);
    let result = result.unwrap();
    assert!(
        result.is_success(),
        "shell_exec should succeed for whitelisted command: {}",
        result.content
    );
    assert!(
        result.content.contains("hello"),
        "Should output 'hello', got: {}",
        result.content
    );
}

#[tokio::test]
async fn test_shell_tools_blocked_command() {
    let mut config = ShellConfig::default();
    config.enabled = true;
    config.allowed_commands = vec!["echo".to_string()];

    let tools = create_shell_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Try non-whitelisted command
    // Note: tool name is "shell_exec"
    let args = serde_json::json!({
        "command": "ls -la"
    });

    let result = registry
        .execute("shell_exec", &args.to_string())
        .await;
    // Command validation returns an error when not in whitelist
    assert!(
        result.is_err(),
        "shell_exec should return error for non-whitelisted command"
    );
}

// =============================================================================
// System Tools Integration Tests
// =============================================================================

#[tokio::test]
async fn test_system_info_tool() {
    let tools = create_system_tools();

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // Test sys_info (note: tool name is "sys_info", not "system_info")
    let args = serde_json::json!({});

    let result = registry
        .execute("sys_info", &args.to_string())
        .await;
    assert!(result.is_ok(), "sys_info should succeed: {:?}", result);
    let result = result.unwrap();
    assert!(result.is_success(), "sys_info should return success: {}", result.content);

    // Should contain system information
    let content = result.content.to_lowercase();
    assert!(
        content.contains("os") || content.contains("system") || content.contains("darwin") || content.contains("mac"),
        "Should contain OS information, got: {}",
        content
    );
}

// =============================================================================
// Tool Registry Integration Tests
// =============================================================================

#[tokio::test]
async fn test_registry_multiple_tool_types() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    let registry = NativeToolRegistry::new();

    // Register filesystem tools
    let fs_config = FilesystemConfig::new(vec![temp_path]);
    registry.register_all(create_filesystem_tools(fs_config)).await;

    // Register git tools
    let git_config = GitConfig::default();
    registry.register_all(create_git_tools(git_config)).await;

    // Register system tools
    registry.register_all(create_system_tools()).await;

    // Should have multiple tools
    let count = registry.count().await;
    assert!(count >= 10, "Should have at least 10 tools registered");

    // Check tools by category
    let fs_defs = registry
        .get_definitions_by_category(ToolCategory::Native)
        .await;
    assert!(!fs_defs.is_empty(), "Should have filesystem tools");

    let git_defs = registry
        .get_definitions_by_category(ToolCategory::Native)
        .await;
    assert!(!git_defs.is_empty(), "Should have git tools");

    let sys_defs = registry
        .get_definitions_by_category(ToolCategory::Builtin)
        .await;
    assert!(!sys_defs.is_empty(), "Should have builtin tools");
}

#[tokio::test]
async fn test_registry_openai_format() {
    let registry = NativeToolRegistry::new();
    registry.register_all(create_system_tools()).await;

    let openai_tools = registry.to_openai_tools().await;

    assert!(!openai_tools.is_empty(), "Should have tools");

    // Verify OpenAI format
    for tool in &openai_tools {
        assert_eq!(tool["type"], "function", "Type should be 'function'");
        assert!(
            tool["function"]["name"].is_string(),
            "Should have function name"
        );
        assert!(
            tool["function"]["description"].is_string(),
            "Should have function description"
        );
        assert!(
            tool["function"]["parameters"].is_object(),
            "Should have function parameters"
        );
    }
}

#[tokio::test]
async fn test_registry_anthropic_format() {
    let registry = NativeToolRegistry::new();
    registry.register_all(create_system_tools()).await;

    let anthropic_tools = registry.to_anthropic_tools().await;

    assert!(!anthropic_tools.is_empty(), "Should have tools");

    // Verify Anthropic format
    for tool in &anthropic_tools {
        assert!(tool["name"].is_string(), "Should have name");
        assert!(tool["description"].is_string(), "Should have description");
        assert!(
            tool["input_schema"].is_object(),
            "Should have input_schema"
        );
    }
}

#[tokio::test]
async fn test_registry_tool_not_found() {
    let registry = NativeToolRegistry::new();
    registry.register_all(create_system_tools()).await;

    let result = registry.execute("nonexistent_tool", "{}").await;

    assert!(result.is_err(), "Should return error for unknown tool");
    match result {
        Err(aethecore::AetherError::ToolNotFound { name, .. }) => {
            assert_eq!(name, "nonexistent_tool");
        }
        _ => panic!("Expected ToolNotFound error"),
    }
}

#[tokio::test]
async fn test_registry_confirmation_tools() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    let config = FilesystemConfig::new(vec![temp_path]);
    let tools = create_filesystem_tools(config);

    let registry = NativeToolRegistry::new();
    registry.register_all(tools).await;

    // file_write should require confirmation
    let write_confirmation = registry.requires_confirmation("file_write").await;
    assert_eq!(
        write_confirmation,
        Some(true),
        "file_write should require confirmation"
    );

    // file_read should not require confirmation
    let read_confirmation = registry.requires_confirmation("file_read").await;
    assert_eq!(
        read_confirmation,
        Some(false),
        "file_read should not require confirmation"
    );

    // Get all confirmation tools
    let confirmation_tools = registry.get_confirmation_tools().await;
    assert!(
        confirmation_tools.iter().any(|t| t.name == "file_write"),
        "file_write should be in confirmation tools"
    );
    assert!(
        confirmation_tools.iter().any(|t| t.name == "file_delete"),
        "file_delete should be in confirmation tools"
    );
}
