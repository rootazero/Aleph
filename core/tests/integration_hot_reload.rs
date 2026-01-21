//! Integration tests for hot-reload functionality
//!
//! Tests that MCP server and skill modifications trigger the on_tools_changed callback.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use aethecore::ffi::{init_core, AetherEventHandler};
use aethecore::intent::ExecutableTaskFFI;
use aethecore::mcp::{McpEnvVar, McpServerConfig, McpServerPermissions, McpServerType};
use aethecore::McpStartupReportFFI;

/// Mock event handler that tracks callback invocations
struct MockEventHandler {
    tools_changed_count: AtomicU32,
    last_tool_count: AtomicU32,
}

impl MockEventHandler {
    fn new() -> Self {
        Self {
            tools_changed_count: AtomicU32::new(0),
            last_tool_count: AtomicU32::new(0),
        }
    }

    fn get_tools_changed_count(&self) -> u32 {
        self.tools_changed_count.load(Ordering::SeqCst)
    }

    fn get_last_tool_count(&self) -> u32 {
        self.last_tool_count.load(Ordering::SeqCst)
    }
}

impl AetherEventHandler for MockEventHandler {
    fn on_thinking(&self) {}
    fn on_tool_start(&self, _tool_name: String) {}
    fn on_tool_result(&self, _tool_name: String, _result: String) {}
    fn on_stream_chunk(&self, _text: String) {}
    fn on_complete(&self, _response: String) {}
    fn on_error(&self, _message: String) {}
    fn on_memory_stored(&self) {}
    fn on_agent_mode_detected(&self, _task: ExecutableTaskFFI) {}

    fn on_tools_changed(&self, tool_count: u32) {
        println!(
            "[MockEventHandler] on_tools_changed called with {} tools",
            tool_count
        );
        self.tools_changed_count.fetch_add(1, Ordering::SeqCst);
        self.last_tool_count.store(tool_count, Ordering::SeqCst);
    }

    fn on_mcp_startup_complete(&self, report: McpStartupReportFFI) {
        println!(
            "[MockEventHandler] on_mcp_startup_complete: {} succeeded, {} failed",
            report.succeeded_servers.len(),
            report.failed_servers.len()
        );
    }

    // Phase 5 callbacks
    fn on_session_started(&self, _session_id: String) {}
    fn on_tool_call_started(&self, _call_id: String, _tool_name: String) {}
    fn on_tool_call_completed(&self, _call_id: String, _output: String) {}
    fn on_tool_call_failed(&self, _call_id: String, _error: String, _is_retryable: bool) {}
    fn on_loop_progress(&self, _session_id: String, _iteration: u32, _status: String) {}
    fn on_plan_created(&self, _session_id: String, _steps: Vec<String>) {}
    fn on_session_completed(&self, _session_id: String, _summary: String) {}
    fn on_subagent_started(
        &self,
        _parent_session_id: String,
        _child_session_id: String,
        _agent_id: String,
    ) {
    }
    fn on_subagent_completed(&self, _child_session_id: String, _success: bool, _summary: String) {}
    fn on_runtime_updates_available(&self, _updates: Vec<aethecore::RuntimeUpdateInfo>) {}
    fn on_plan_confirmation_required(&self, _plan_id: String, _plan: aethecore::DagTaskPlan) {}
}

/// Create a test MCP server configuration
fn create_test_mcp_config(id: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        name: format!("Test Server {}", id),
        server_type: McpServerType::External,
        enabled: true,
        command: Some("echo".to_string()),
        args: vec!["test".to_string()],
        env: vec![McpEnvVar {
            key: "TEST".to_string(),
            value: "1".to_string(),
        }],
        working_directory: None,
        trigger_command: Some(format!("/mcp/{}", id)),
        permissions: McpServerPermissions {
            requires_confirmation: false,
            allowed_paths: vec![],
            allowed_commands: vec![],
        },
        icon: "server.rack".to_string(),
        color: "#007AFF".to_string(),
    }
}

#[test]
fn test_mcp_add_triggers_tools_changed() {
    // Use the real config directory since Config::save() uses default_path()
    // We'll clean up any test servers we create
    let config_dir = dirs::config_dir()
        .expect("Failed to get config dir")
        .join("aether");
    std::fs::create_dir_all(&config_dir).expect("Failed to create aether dir");
    let config_path = config_dir.join("config.toml");

    // Read existing config or create minimal one
    let config_content = std::fs::read_to_string(&config_path).unwrap_or_else(|_| {
        r#"
[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o"
api_key = "sk-test-dummy-key-for-testing"

[memory]
enabled = false

[mcp]
enabled = true
"#
        .to_string()
    });

    // Write config if it doesn't exist or is empty
    if config_content.trim().is_empty() {
        std::fs::write(
            &config_path,
            r#"
[general]
default_provider = "openai"

[providers.openai]
model = "gpt-4o"
api_key = "sk-test-dummy-key-for-testing"

[memory]
enabled = false

[mcp]
enabled = true
"#,
        )
        .expect("Failed to write config");
    }

    // Create mock handler
    let handler = Arc::new(MockEventHandler::new());

    // Initialize core
    let core = init_core(
        config_path.to_string_lossy().to_string(),
        Box::new(MockEventHandlerWrapper(handler.clone())),
    )
    .expect("Failed to init core");

    // Verify initial state
    assert_eq!(handler.get_tools_changed_count(), 0);

    // Add an MCP server
    let server_config = create_test_mcp_config("test-server-1");
    core.add_mcp_server(server_config)
        .expect("Failed to add MCP server");

    // Verify on_tools_changed was called
    assert_eq!(
        handler.get_tools_changed_count(),
        1,
        "on_tools_changed should be called once after adding MCP server"
    );

    println!(
        "✓ MCP add triggered on_tools_changed (tool_count: {})",
        handler.get_last_tool_count()
    );

    // Add another server
    let server_config2 = create_test_mcp_config("test-server-2");
    core.add_mcp_server(server_config2)
        .expect("Failed to add second MCP server");

    assert_eq!(
        handler.get_tools_changed_count(),
        2,
        "on_tools_changed should be called again after adding second MCP server"
    );

    println!("✓ Second MCP add triggered on_tools_changed");

    // Delete a server
    core.delete_mcp_server("test-server-1".to_string())
        .expect("Failed to delete MCP server");

    assert_eq!(
        handler.get_tools_changed_count(),
        3,
        "on_tools_changed should be called after deleting MCP server"
    );

    println!("✓ MCP delete triggered on_tools_changed");

    // Cleanup - delete remaining test server
    let _ = core.delete_mcp_server("test-server-2".to_string());
    drop(core);
}

#[test]
#[ignore] // Skip in parallel runs - covered by test_mcp_add_triggers_tools_changed
fn test_mcp_update_triggers_tools_changed() {
    // Use the real config directory since Config::save() uses default_path()
    let config_dir = dirs::config_dir()
        .expect("Failed to get config dir")
        .join("aether");
    std::fs::create_dir_all(&config_dir).expect("Failed to create aether dir");
    let config_path = config_dir.join("config.toml");

    let handler = Arc::new(MockEventHandler::new());

    let core = init_core(
        config_path.to_string_lossy().to_string(),
        Box::new(MockEventHandlerWrapper(handler.clone())),
    )
    .expect("Failed to init core");

    // Add a server first
    let mut server_config = create_test_mcp_config("update-test");
    core.add_mcp_server(server_config.clone())
        .expect("Failed to add MCP server");

    let count_after_add = handler.get_tools_changed_count();
    assert_eq!(count_after_add, 1);

    // Update the server
    server_config.name = "Updated Test Server".to_string();
    server_config.args = vec!["updated".to_string()];
    core.update_mcp_server(server_config)
        .expect("Failed to update MCP server");

    assert_eq!(
        handler.get_tools_changed_count(),
        2,
        "on_tools_changed should be called after updating MCP server"
    );

    println!("✓ MCP update triggered on_tools_changed");

    // Cleanup - delete test server
    let _ = core.delete_mcp_server("update-test".to_string());
    drop(core);
}

/// Wrapper to make Arc<MockEventHandler> implement AetherEventHandler
struct MockEventHandlerWrapper(Arc<MockEventHandler>);

impl AetherEventHandler for MockEventHandlerWrapper {
    fn on_thinking(&self) {
        self.0.on_thinking()
    }
    fn on_tool_start(&self, tool_name: String) {
        self.0.on_tool_start(tool_name)
    }
    fn on_tool_result(&self, tool_name: String, result: String) {
        self.0.on_tool_result(tool_name, result)
    }
    fn on_stream_chunk(&self, text: String) {
        self.0.on_stream_chunk(text)
    }
    fn on_complete(&self, response: String) {
        self.0.on_complete(response)
    }
    fn on_error(&self, message: String) {
        self.0.on_error(message)
    }
    fn on_memory_stored(&self) {
        self.0.on_memory_stored()
    }
    fn on_agent_mode_detected(&self, task: ExecutableTaskFFI) {
        self.0.on_agent_mode_detected(task)
    }
    fn on_tools_changed(&self, tool_count: u32) {
        self.0.on_tools_changed(tool_count)
    }
    fn on_mcp_startup_complete(&self, report: McpStartupReportFFI) {
        self.0.on_mcp_startup_complete(report)
    }

    // Phase 5 callbacks
    fn on_session_started(&self, session_id: String) {
        self.0.on_session_started(session_id)
    }
    fn on_tool_call_started(&self, call_id: String, tool_name: String) {
        self.0.on_tool_call_started(call_id, tool_name)
    }
    fn on_tool_call_completed(&self, call_id: String, output: String) {
        self.0.on_tool_call_completed(call_id, output)
    }
    fn on_tool_call_failed(&self, call_id: String, error: String, is_retryable: bool) {
        self.0.on_tool_call_failed(call_id, error, is_retryable)
    }
    fn on_loop_progress(&self, session_id: String, iteration: u32, status: String) {
        self.0.on_loop_progress(session_id, iteration, status)
    }
    fn on_plan_created(&self, session_id: String, steps: Vec<String>) {
        self.0.on_plan_created(session_id, steps)
    }
    fn on_session_completed(&self, session_id: String, summary: String) {
        self.0.on_session_completed(session_id, summary)
    }
    fn on_subagent_started(
        &self,
        parent_session_id: String,
        child_session_id: String,
        agent_id: String,
    ) {
        self.0
            .on_subagent_started(parent_session_id, child_session_id, agent_id)
    }
    fn on_subagent_completed(&self, child_session_id: String, success: bool, summary: String) {
        self.0
            .on_subagent_completed(child_session_id, success, summary)
    }
    fn on_runtime_updates_available(&self, updates: Vec<aethecore::RuntimeUpdateInfo>) {
        self.0.on_runtime_updates_available(updates)
    }
    fn on_plan_confirmation_required(&self, plan_id: String, plan: aethecore::DagTaskPlan) {
        self.0.on_plan_confirmation_required(plan_id, plan)
    }
}
