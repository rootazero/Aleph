# Tasks: Introduce Native Function Calling Architecture

## Phase 1: Core Infrastructure ✅ COMPLETED

### 1.1 Create AgentTool Trait Module
- [x] 1.1.1 Create `core/src/tools/mod.rs` with module structure
- [x] 1.1.2 Create `core/src/tools/traits.rs` with `AgentTool` trait
- [x] 1.1.3 Define `ToolDefinition` struct with JSON Schema support
- [x] 1.1.4 Define `ToolResult` struct with success/error variants
- [x] 1.1.5 Define `ToolCategory` enum for UI grouping
- [x] 1.1.6 Add unit tests for trait definitions (12 tests)

### 1.2 Create Tool Registry Integration
- [x] 1.2.1 Create `core/src/tools/registry.rs` with native tool storage
- [x] 1.2.2 Implement `register_native()` for `Arc<dyn AgentTool>`
- [x] 1.2.3 Implement `execute()` method for tool invocation
- [x] 1.2.4 Add `get_definitions()` for LLM tool list generation
- [x] 1.2.5 Add `to_openai_tools()` and `to_anthropic_tools()` format converters
- [x] 1.2.6 Add unit tests for registry operations (16 tests)

## Phase 2: Filesystem Tools Migration ✅ COMPLETED

### 2.1 File Operations
- [x] 2.1.1 Create `core/src/tools/filesystem/mod.rs` with module structure
- [x] 2.1.2 Implement `FileReadTool` with `AgentTool` trait (4 tests)
- [x] 2.1.3 Implement `FileWriteTool` with confirmation support (5 tests)
- [x] 2.1.4 Implement `FileListTool` for directory listing (4 tests)
- [x] 2.1.5 Implement `FileDeleteTool` with confirmation support (5 tests)
- [x] 2.1.6 Implement `FileSearchTool` with glob patterns (4 tests)
- [x] 2.1.7 Add `FilesystemConfig` and `FilesystemContext` for shared path security (5 tests)
- [x] 2.1.8 Add `create_all_tools()` convenience function (2 tests)

## Phase 3: Git Tools Migration ✅ COMPLETED

### 3.1 Git Operations
- [x] 3.1.1 Create `core/src/tools/git/mod.rs`
- [x] 3.1.2 Implement `GitStatusTool` (4 tests)
- [x] 3.1.3 Implement `GitDiffTool` (5 tests)
- [x] 3.1.4 Implement `GitLogTool` (5 tests)
- [x] 3.1.5 Implement `GitBranchTool` (5 tests)
- [x] 3.1.6 Add `GitConfig` and `GitContext` for repository path validation (5 tests)
- [x] 3.1.7 Add `create_all_tools()` convenience function (3 tests)

## Phase 4: Shell Tools Migration ✅ COMPLETED

### 4.1 Shell Command Execution
- [x] 4.1.1 Create `core/src/tools/shell/mod.rs` with module structure
- [x] 4.1.2 Implement `ShellExecuteTool` with timeout support (7 tests)
- [x] 4.1.3 Add `ShellConfig` with command whitelist/blacklist validation (10 tests)
- [x] 4.1.4 Add `ShellContext` for shared configuration
- [x] 4.1.5 Add `create_all_tools()` convenience function (4 tests)
- [x] 4.1.6 Security features: disabled by default, dangerous commands blocked, confirmation required

## Phase 5: Utility Tools Migration ✅ COMPLETED

### 5.1 System Information
- [x] 5.1.1 Create `core/src/tools/system/mod.rs`
- [x] 5.1.2 Implement `SystemInfoTool` for OS/hardware info (6 tests)
- [x] 5.1.3 Add unit tests for system tools

### 5.2 Clipboard Operations
- [x] 5.2.1 Create `core/src/tools/clipboard/mod.rs`
- [x] 5.2.2 Implement `ClipboardReadTool` (6 tests)
- [x] 5.2.3 Add unit tests for clipboard tools

### 5.3 Screen Capture
- [x] 5.3.1 Create `core/src/tools/screen/mod.rs`
- [x] 5.3.2 Implement `ScreenCaptureTool` (6 tests, always requires confirmation)
- [x] 5.3.3 Add unit tests for screen tools

### 5.4 Web Search
- [x] 5.4.1 Create `core/src/tools/search/mod.rs`
- [x] 5.4.2 Implement `WebSearchTool` with PII scrubbing (9 tests)
- [x] 5.4.3 Add unit tests for search tools

## Phase 6: MCP Bridge for External Servers ✅ COMPLETED

### 6.1 MCP Tool Bridge
- [x] 6.1.1 Create `core/src/mcp/bridge.rs` with `McpToolBridge` struct
- [x] 6.1.2 Implement `AgentTool` trait for `McpToolBridge`
- [x] 6.1.3 Bridge `execute()` to existing MCP JSON-RPC calls (via McpClient::call_tool)
- [x] 6.1.4 Add unit tests for MCP bridge (16 tests)

## Phase 7: Integration and Cleanup ⏳ IN PROGRESS

### 7.1 Dispatcher Integration
- [x] 7.1.1 Update `dispatcher/registry.rs` to use `AgentTool`
  - Added `register_agent_tools()` method for registering `Arc<dyn AgentTool>` instances
  - Added `refresh_with_agent_tools()` as new preferred refresh method
- [x] 7.1.2 Add `ToolSource::Native` for AgentTool registrations (kept backward compatibility)
- [x] 7.1.3 Update `UnifiedTool` to derive from `ToolDefinition`
  - Added `from_tool_definition()` method with `icon_for_category()` helper
- [x] 7.1.4 Add integration test for AgentTools registration flow

### 7.2 Remove Old SystemTool Infrastructure (DEFERRED - Backward Compatibility)
> **Note**: Old SystemTool infrastructure kept for backward compatibility.
> Both `refresh_all()` (SystemTool) and `refresh_with_agent_tools()` (AgentTool) are available.
> Cleanup can be done in a future PR after Swift UI fully migrates to new API.

- [ ] 7.2.1 Remove `services/tools/traits.rs` (SystemTool trait)
- [ ] 7.2.2 Remove `services/tools/fs_tool.rs`
- [ ] 7.2.3 Remove `services/tools/git_tool.rs`
- [ ] 7.2.4 Remove `services/tools/shell_tool.rs`
- [ ] 7.2.5 Remove `services/tools/sys_tool.rs`
- [ ] 7.2.6 Remove `services/tools/clipboard_tool.rs`
- [ ] 7.2.7 Remove `services/tools/screen_tool.rs`
- [ ] 7.2.8 Remove `services/tools/search_tool.rs`
- [ ] 7.2.9 Update `services/tools/mod.rs` exports
- [ ] 7.2.10 Update `mcp/mod.rs` to remove SystemTool re-exports

### 7.3 Core Module Updates ✅ COMPLETED
- [x] 7.3.1 Update `core.rs` to initialize new tool registry
  - Added `NativeToolRegistry` field to `AetherCore`
  - Updated `refresh_tool_registry()` to create and register native AgentTools
  - Added `create_native_agent_tools()` helper method
  - Added `group_native_tools_by_service()` for dispatcher registration
- [x] 7.3.2 Update `lib.rs` exports for new modules
  - Added `create_*_tools` convenience functions to exports
  - Organized exports by tool category with comments
- [x] 7.3.3 Add `tools` module to `Cargo.toml` if needed
  - No changes needed - dependencies already present (glob, etc.)
- [x] 7.3.4 Update UniFFI `.udl` file if needed
  - No changes needed now - existing tool management methods sufficient
  - Native tool execution methods (`execute_native_tool`, etc.) can be exported later
- [x] 7.3.5 Add native tool execution methods to AetherCore
  - `execute_native_tool(name, args)` - Execute tool by name
  - `native_tool_requires_confirmation(name)` - Check if confirmation needed
  - `get_native_tool_definitions()` - Get all tool definitions
  - `get_native_tools_openai()` / `get_native_tools_anthropic()` - Format converters
  - `native_tool_count()` - Get registered tool count

## Phase 8: Testing and Documentation ✅ COMPLETED

### 8.1 Integration Tests
- [x] 8.1.1 Add integration tests for filesystem operations
  - Created `tests/integration_native_tools.rs`
  - Tests: file_read_write, file_list, file_search, file_delete, permission_denied
- [x] 8.1.2 Add integration tests for git operations
  - Tests: git_status, git_branch, git_log, git in non-repo
- [x] 8.1.3 Add integration tests for shell execution
  - Tests: disabled_by_default, enabled_with_whitelist, blocked_command
- [x] 8.1.4 Add integration tests for tool registry
  - Tests: multiple_tool_types, openai_format, anthropic_format, tool_not_found, confirmation_tools
- [x] 8.1.5 Verify all existing functionality preserved
  - All 1109 unit tests pass
  - 17 new integration tests pass

### 8.2 Documentation
- [x] 8.2.1 Update CLAUDE.md with new architecture description
  - Tools module already documented in existing CLAUDE.md
- [x] 8.2.2 Add code comments for AgentTool trait
  - `tools/traits.rs` has comprehensive documentation
- [x] 8.2.3 Update module-level documentation
  - `tools/mod.rs` has architecture diagram and examples

## Verification Checklist

- [x] All `cargo test` passes (1109 tests + 17 integration tests)
- [x] All `cargo clippy` warnings resolved (removed unused ToolPriority import)
- [x] `cargo build --release` succeeds
- [ ] Swift UI still displays tools correctly (requires manual testing)
- [ ] Tool execution works end-to-end (requires manual testing)
- [ ] MCP external servers still function (requires manual testing)
- [x] No regression in existing features (verified via tests)
