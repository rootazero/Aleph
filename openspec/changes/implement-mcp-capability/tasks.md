# Tasks: Implement MCP Capability

## Phase 0: Shared Foundation ⭐⭐

**目标**：建立共享基础模块 (`services/`)，供 MCP、Skills 及未来扩展复用

### 0.1 模块结构
- [ ] 0.1.1 Create `Aleph/core/src/services/mod.rs` module root
- [ ] 0.1.2 Define module exports for fs, git, system_info

### 0.2 FileOps Trait (services::fs)
- [ ] 0.2.1 Create `Aleph/core/src/services/fs/mod.rs`
- [ ] 0.2.2 Define `DirEntry` struct (name, path, is_dir, size, modified)
- [ ] 0.2.3 Define `FileOps` trait with methods:
  - `read_file()`, `read_file_bytes()`
  - `write_file()`, `write_file_bytes()`
  - `list_dir()`, `exists()`, `is_dir()`
  - `create_dir()`, `delete()`
  - `search()` (glob pattern)
- [ ] 0.2.4 Create `Aleph/core/src/services/fs/local.rs`
- [ ] 0.2.5 Implement `LocalFs` using `tokio::fs`
- [ ] 0.2.6 Add `glob` crate for pattern matching
- [ ] 0.2.7 Write unit tests for FileOps

### 0.3 GitOps Trait (services::git)
- [ ] 0.3.1 Create `Aleph/core/src/services/git/mod.rs`
- [ ] 0.3.2 Define data structs: `GitFileStatus`, `GitCommit`, `GitDiff`
- [ ] 0.3.3 Define `GitOps` trait with methods:
  - `status()`, `log()`, `diff()`
  - `current_branch()`, `is_repo()`
- [ ] 0.3.4 Add `git2` to Cargo.toml dependencies
- [ ] 0.3.5 Create `Aleph/core/src/services/git/repository.rs`
- [ ] 0.3.6 Implement `GitRepository` using `git2-rs`
- [ ] 0.3.7 Wrap all git2 calls with `tokio::task::spawn_blocking`
- [ ] 0.3.8 Write unit tests for GitOps

### 0.4 SystemInfoProvider Trait (services::system_info)
- [ ] 0.4.1 Create `Aleph/core/src/services/system_info/mod.rs`
- [ ] 0.4.2 Define `SystemInfo` struct (os_name, os_version, hostname, etc.)
- [ ] 0.4.3 Define `SystemInfoProvider` trait with methods:
  - `get_info()`, `active_application()`, `active_window_title()`
- [ ] 0.4.4 Create `Aleph/core/src/services/system_info/macos.rs`
- [ ] 0.4.5 Implement `MacOsSystemInfo` using system commands / CoreFoundation
- [ ] 0.4.6 Write unit tests for SystemInfoProvider

### 0.5 集成验证
- [ ] 0.5.1 Update `lib.rs` to export services module
- [ ] 0.5.2 Write integration tests for all services
- [ ] 0.5.3 Verify services can be used independently of MCP

---

## Phase 1: Builtin Services (MVP) ⭐

**目标**：用户零配置即可使用核心 MCP 功能，无需安装 Node.js/Python

### 1.1 模块结构与数据类型
- [ ] 1.1.1 Create `Aleph/core/src/mcp/mod.rs` module root
- [ ] 1.1.2 Create `Aleph/core/src/mcp/types.rs` with `McpResource`, `McpTool`, `McpToolResult`
- [ ] 1.1.3 Create `Aleph/core/src/mcp/config.rs` with `McpConfig`, `BuiltinConfig`, `McpPermissions`
- [ ] 1.1.4 Update `AgentContext.mcp_resources` to `mcp_context: Option<McpContext>`
- [ ] 1.1.5 Add `McpContext` struct with `resources`, `tool_results`, `available_tools` fields
- [ ] 1.1.6 Write unit tests for data structures (serialization/deserialization)

### 1.2 BuiltinMcpService Trait (MCP 适配器接口)
- [ ] 1.2.1 Create `Aleph/core/src/mcp/builtin/mod.rs`
- [ ] 1.2.2 Define `BuiltinMcpService` trait with methods:
  - `name()`, `description()`
  - `list_resources()`, `read_resource(uri)`
  - `list_tools()`, `call_tool(name, args)`
  - `requires_confirmation(tool_name)`
- [ ] 1.2.3 Write mock implementation for testing

### 1.3 FsService (MCP 适配器 - 封装 services::fs)
- [ ] 1.3.1 Create `Aleph/core/src/mcp/builtin/fs.rs`
- [ ] 1.3.2 Implement `FsService` struct with `Arc<dyn FileOps>` + `allowed_roots`
- [ ] 1.3.3 Add `with_fs()` constructor for dependency injection (testing)
- [ ] 1.3.4 Implement `is_path_allowed()` security check
- [ ] 1.3.5 Implement `file_read` tool delegating to `self.fs.read_file()`
- [ ] 1.3.6 Implement `file_write` tool delegating to `self.fs.write_file()`
- [ ] 1.3.7 Implement `file_list` tool delegating to `self.fs.list_dir()`
- [ ] 1.3.8 Implement `file_search` tool delegating to `self.fs.search()`
- [ ] 1.3.9 Write unit tests for FsService (with mock FileOps)

### 1.4 GitService (MCP 适配器 - 封装 services::git)
- [ ] 1.4.1 Create `Aleph/core/src/mcp/builtin/git.rs`
- [ ] 1.4.2 Implement `GitService` struct with `Arc<dyn GitOps>` + `allowed_repos`
- [ ] 1.4.3 Add `with_git()` constructor for dependency injection (testing)
- [ ] 1.4.4 Implement `is_repo_allowed()` security check
- [ ] 1.4.5 Implement `git_status` tool delegating to `self.git.status()`
- [ ] 1.4.6 Implement `git_log` tool delegating to `self.git.log()`
- [ ] 1.4.7 Implement `git_diff` tool delegating to `self.git.diff()`
- [ ] 1.4.8 Write unit tests for GitService (with mock GitOps)

### 1.5 SystemInfoService (MCP 适配器 - 封装 services::system_info)
- [ ] 1.5.1 Create `Aleph/core/src/mcp/builtin/system_info.rs`
- [ ] 1.5.2 Implement `SystemInfoService` struct with `Arc<dyn SystemInfoProvider>`
- [ ] 1.5.3 Implement `sys_info` tool delegating to `self.provider.get_info()`
- [ ] 1.5.4 Implement `active_app` tool delegating to `self.provider.active_application()`
- [ ] 1.5.5 Write unit tests for SystemInfoService

### 1.6 ShellService (独立实现 - 不使用共享基础模块)
- [ ] 1.6.1 Create `Aleph/core/src/mcp/builtin/shell.rs`
- [ ] 1.6.2 Implement `ShellService` struct with `timeout`, `allowed_commands` fields
- [ ] 1.6.3 Implement `is_command_allowed()` whitelist check
- [ ] 1.6.4 Implement `shell_exec` tool using `tokio::process::Command`
- [ ] 1.6.5 Add timeout protection with `tokio::time::timeout`
- [ ] 1.6.6 Capture stdout/stderr and exit code
- [ ] 1.6.7 Write unit tests for ShellService

### 1.7 McpClient (服务注册与路由)
- [ ] 1.7.1 Create `Aleph/core/src/mcp/client.rs`
- [ ] 1.7.2 Implement `McpClient` struct with `builtin_services` HashMap
- [ ] 1.7.3 Implement `register_builtin()` for service registration
- [ ] 1.7.4 Implement `list_tools()` aggregating from all builtin services
- [ ] 1.7.5 Implement `call_tool()` with routing to correct service
- [ ] 1.7.6 Implement `requires_confirmation()` check
- [ ] 1.7.7 Write unit tests for McpClient

### 1.8 McpStrategy 实现
- [ ] 1.8.1 Update `McpStrategy` to accept `Option<Arc<McpClient>>`
- [ ] 1.8.2 Implement `execute()` method:
  - List available tools
  - Populate `payload.context.mcp_context`
- [ ] 1.8.3 Update `is_available()` to check client presence
- [ ] 1.8.4 Update `CapabilityExecutor` to initialize McpClient
- [ ] 1.8.5 Write unit tests with mock client

### 1.9 PromptAssembler 扩展
- [ ] 1.9.1 Add `format_mcp_context_markdown()` method
- [ ] 1.9.2 Format available tools as markdown list
- [ ] 1.9.3 Format tool results with status and output
- [ ] 1.9.4 Integrate into `assemble_system_prompt()` flow
- [ ] 1.9.5 Write formatting tests

### 1.10 配置解析
- [ ] 1.10.1 Update main `Config` struct to include `mcp: Option<McpConfig>`
- [ ] 1.10.2 Implement `BuiltinConfig` parsing (fs, git, shell, system_info)
- [ ] 1.10.3 Handle `~` path expansion for `allowed_roots`/`allowed_repos`
- [ ] 1.10.4 Write config parsing tests with sample TOML

### 1.11 权限控制
- [ ] 1.11.1 Implement `dangerous_tools` config parsing
- [ ] 1.11.2 Implement confirmation check before calling dangerous tools
- [ ] 1.11.3 Add `ToolConfirmationRequired` callback in event handler
- [ ] 1.11.4 Write permission tests

## Phase 2: External Server Support

**目标**：支持高级用户配置外部 MCP Server（带运行时检测）

### 2.1 JSON-RPC 2.0 协议
- [ ] 2.1.1 Create `Aleph/core/src/mcp/jsonrpc.rs`
- [ ] 2.1.2 Implement `JsonRpcRequest` struct with builder pattern
- [ ] 2.1.3 Implement `JsonRpcResponse` with `Success` and `Error` variants
- [ ] 2.1.4 Implement `IdGenerator` for request ID management
- [ ] 2.1.5 Write unit tests for JSON-RPC serialization

### 2.2 Stdio Transport
- [ ] 2.2.1 Create `Aleph/core/src/mcp/transport/mod.rs`
- [ ] 2.2.2 Create `Aleph/core/src/mcp/transport/stdio.rs`
- [ ] 2.2.3 Implement `StdioTransport::spawn()` for subprocess creation
- [ ] 2.2.4 Implement `StdioTransport::send()` for request/response
- [ ] 2.2.5 Implement `StdioTransport::close()` for cleanup
- [ ] 2.2.6 Add timeout handling for unresponsive servers
- [ ] 2.2.7 Write integration tests with mock MCP server

### 2.3 运行时检测
- [ ] 2.3.1 Implement `check_runtime()` function (node/python/bun)
- [ ] 2.3.2 Add `requires_runtime` field to `ExternalServerConfig`
- [ ] 2.3.3 Skip server startup if runtime not available
- [ ] 2.3.4 Add user-friendly warning message via event handler
- [ ] 2.3.5 Write tests for runtime detection

### 2.4 McpServerConnection
- [ ] 2.4.1 Create `Aleph/core/src/mcp/external/mod.rs`
- [ ] 2.4.2 Create `Aleph/core/src/mcp/external/connection.rs`
- [ ] 2.4.3 Implement `connect()` with MCP `initialize` handshake
- [ ] 2.4.4 Implement `list_tools()` → `tools/list` RPC
- [ ] 2.4.5 Implement `has_tool()` check
- [ ] 2.4.6 Implement `call_tool()` → `tools/call` RPC
- [ ] 2.4.7 Implement `close()` for graceful shutdown
- [ ] 2.4.8 Write unit tests for connection lifecycle

### 2.5 McpClient 扩展
- [ ] 2.5.1 Add `external_servers` field to `McpClient`
- [ ] 2.5.2 Implement `start_external_servers()` with runtime check
- [ ] 2.5.3 Update `list_tools()` to aggregate external servers
- [ ] 2.5.4 Update `call_tool()` to route to external servers
- [ ] 2.5.5 Implement `stop_all()` for cleanup
- [ ] 2.5.6 Write integration tests for multi-server scenarios

## Phase 3: UI & AlephCore Integration

**目标**：完善用户界面和 Core 集成

### 3.1 AlephCore 集成
- [ ] 3.1.1 Initialize `McpClient` in `AlephCore::new()` if configured
- [ ] 3.1.2 Register builtin services based on config
- [ ] 3.1.3 Start external servers on `start_listening()`
- [ ] 3.1.4 Stop all servers on `stop_listening()`
- [ ] 3.1.5 Pass `McpClient` to `CapabilityExecutor`

### 3.2 UniFFI 接口
- [ ] 3.2.1 Add `McpServiceInfo` struct to `aleph.udl`
- [ ] 3.2.2 Add `list_mcp_services()` method to `AlephCore`
- [ ] 3.2.3 Add `get_mcp_tools()` method
- [ ] 3.2.4 Add `ToolConfirmationCallback` interface
- [ ] 3.2.5 Regenerate Swift bindings

### 3.3 Swift UI - McpSettingsView
- [ ] 3.3.1 Create `Aleph/Sources/Components/Settings/McpSettingsView.swift`
- [ ] 3.3.2 Add MCP enabled toggle
- [ ] 3.3.3 Add builtin services section (fs, git, shell toggles)
- [ ] 3.3.4 Add allowed_roots/allowed_repos path editor
- [ ] 3.3.5 Add external servers section (advanced)
- [ ] 3.3.6 Integrate into main Settings navigation

### 3.4 Tool 确认对话框
- [ ] 3.4.1 Create `Aleph/Sources/Components/Dialogs/ToolConfirmationView.swift`
- [ ] 3.4.2 Design confirmation UI (tool name, args, warning)
- [ ] 3.4.3 Implement UniFFI callback for confirmation requests
- [ ] 3.4.4 Show confirmation in Halo/Popover
- [ ] 3.4.5 Handle user response (allow/deny)

### 3.5 服务生命周期
- [ ] 3.5.1 Add health check for external servers
- [ ] 3.5.2 Implement automatic restart on failure (optional)
- [ ] 3.5.3 Add server status indicator in UI

## Phase 4: Documentation & Testing

### 4.1 文档更新
- [ ] 4.1.1 Update `docs/ARCHITECTURE.md` MCP section
- [ ] 4.1.2 Create `docs/MCP_INTEGRATION.md` usage guide
- [ ] 4.1.3 Add MCP configuration examples to sample config.toml
- [ ] 4.1.4 Document security model and permission system
- [ ] 4.1.5 Document builtin services API

### 4.2 集成测试
- [ ] 4.2.1 Write E2E test: FsService file operations
- [ ] 4.2.2 Write E2E test: GitService repository operations
- [ ] 4.2.3 Write E2E test: ShellService command execution
- [ ] 4.2.4 Write E2E test: permission blocking
- [ ] 4.2.5 Write E2E test: external server connection (mock)

### 4.3 手动测试
- [ ] 4.3.1 Test builtin:fs with various file types
- [ ] 4.3.2 Test builtin:git with real repository
- [ ] 4.3.3 Test builtin:shell with whitelisted commands
- [ ] 4.3.4 Test tool confirmation dialog
- [ ] 4.3.5 Test Settings UI service management

## Phase 5: Bundled Servers (Future)

**目标**：提供开箱即用的高级服务（可选，基于用户需求）

### 5.1 打包基础设施
- [ ] 5.1.1 Setup Bun build environment
- [ ] 5.1.2 Create build script for bundled servers
- [ ] 5.1.3 Define `Aleph.app/Contents/Resources/mcp-servers/` structure
- [ ] 5.1.4 Add bundled server discovery in McpClient

### 5.2 候选服务评估
- [ ] 5.2.1 Evaluate Notion MCP server
- [ ] 5.2.2 Evaluate Slack MCP server
- [ ] 5.2.3 Evaluate Browser/Puppeteer MCP server
- [ ] 5.2.4 Prioritize based on user feedback

### 5.3 打包实现
- [ ] 5.3.1 Package selected servers with Bun compile
- [ ] 5.3.2 Test bundled binaries on macOS
- [ ] 5.3.3 Add to Xcode build process
- [ ] 5.3.4 Update app size documentation
