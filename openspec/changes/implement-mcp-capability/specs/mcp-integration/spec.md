## ADDED Requirements

### Requirement: Shared Foundation Module - FileOps

The system SHALL provide a shared `services::fs` module with `FileOps` trait for file system operations that can be used by MCP, Skills, and future extensions.

#### Scenario: FileOps trait definition
- **WHEN** file system operations are needed
- **THEN** the system SHALL provide `FileOps` trait with methods:
  - `read_file(path)` returning file contents as string
  - `read_file_bytes(path)` returning file contents as bytes
  - `write_file(path, content)` writing string to file
  - `write_file_bytes(path, content)` writing bytes to file
  - `list_dir(path)` returning list of `DirEntry`
  - `exists(path)` checking if path exists
  - `is_dir(path)` checking if path is directory
  - `create_dir(path)` creating directory with parents
  - `delete(path)` deleting file or directory
  - `search(base, pattern)` searching files by glob pattern

#### Scenario: LocalFs implementation
- **WHEN** `LocalFs` is used for file operations
- **THEN** the implementation SHALL use `tokio::fs` for async operations
- **AND** the implementation SHALL use `glob` crate for pattern matching in `search()`
- **AND** all methods SHALL be async and non-blocking

#### Scenario: DirEntry data structure
- **WHEN** `list_dir()` or `search()` returns entries
- **THEN** each `DirEntry` SHALL include:
  - `name` - file or directory name
  - `path` - full path
  - `is_dir` - boolean indicating if directory
  - `size` - file size in bytes
  - `modified` - optional modification time

---

### Requirement: Shared Foundation Module - GitOps

The system SHALL provide a shared `services::git` module with `GitOps` trait for Git operations that can be used by MCP, Skills, and future extensions.

#### Scenario: GitOps trait definition
- **WHEN** Git operations are needed
- **THEN** the system SHALL provide `GitOps` trait with methods:
  - `status(repo_path)` returning list of `GitFileStatus`
  - `log(repo_path, limit)` returning list of `GitCommit`
  - `diff(repo_path, staged)` returning list of `GitDiff`
  - `current_branch(repo_path)` returning branch name
  - `is_repo(path)` checking if path is a git repository

#### Scenario: GitRepository implementation
- **WHEN** `GitRepository` is used for Git operations
- **THEN** the implementation SHALL use `git2-rs` library
- **AND** all blocking git2 calls SHALL be wrapped with `tokio::task::spawn_blocking`
- **AND** the implementation SHALL NOT require git CLI to be installed

#### Scenario: Git data structures
- **WHEN** Git operations return data
- **THEN** `GitFileStatus` SHALL include: `path`, `status`, `staged`
- **AND** `GitCommit` SHALL include: `sha`, `message`, `author`, `email`, `timestamp`
- **AND** `GitDiff` SHALL include: `file_path`, `old_start`, `new_start`, `content`

---

### Requirement: Shared Foundation Module - SystemInfoProvider

The system SHALL provide a shared `services::system_info` module with `SystemInfoProvider` trait for system information queries that can be used by MCP, Skills, and future extensions.

#### Scenario: SystemInfoProvider trait definition
- **WHEN** system information is needed
- **THEN** the system SHALL provide `SystemInfoProvider` trait with methods:
  - `get_info()` returning `SystemInfo` struct
  - `active_application()` returning frontmost application name
  - `active_window_title()` returning active window title

#### Scenario: MacOsSystemInfo implementation
- **WHEN** `MacOsSystemInfo` is used on macOS
- **THEN** the implementation SHALL provide system information
- **AND** blocking system calls SHALL be wrapped with `tokio::task::spawn_blocking`

#### Scenario: SystemInfo data structure
- **WHEN** `get_info()` returns system information
- **THEN** `SystemInfo` SHALL include:
  - `os_name`, `os_version`, `hostname`, `username`
  - `home_dir`, `cpu_arch`
  - `memory_total`, `memory_available` (in bytes)

---

### Requirement: Shared Foundation Module Independence

The shared foundation modules SHALL be independent and usable without MCP.

#### Scenario: Standalone usage
- **WHEN** Skills or future extensions need file/git/system operations
- **THEN** they SHALL be able to import `services::fs`, `services::git`, or `services::system_info` directly
- **AND** they SHALL NOT need to depend on `mcp` module

#### Scenario: Testability
- **WHEN** testing components that use shared foundation modules
- **THEN** mock implementations of `FileOps`, `GitOps`, `SystemInfoProvider` SHALL be injectable
- **AND** components SHALL accept `Arc<dyn TraitName>` for dependency injection

---

### Requirement: Builtin MCP Service Trait

The system SHALL provide a trait for implementing builtin MCP services that run directly in the Rust Core without external dependencies.

#### Scenario: Define builtin service interface
- **WHEN** a builtin MCP service is implemented
- **THEN** the system SHALL provide `BuiltinMcpService` trait with methods:
  - `name()` returning service identifier (e.g., "builtin:fs")
  - `description()` returning human-readable description
  - `list_resources()` returning available resources
  - `read_resource(uri)` returning resource contents
  - `list_tools()` returning available tools
  - `call_tool(name, args)` executing the tool
  - `requires_confirmation(tool_name)` checking if tool needs user confirmation

#### Scenario: Service isolation
- **WHEN** multiple builtin services are registered
- **THEN** each service SHALL operate independently
- **AND** tool names SHALL be unique across services

---

### Requirement: FsService (MCP Adapter for services::fs)

The system SHALL provide a builtin file system MCP service that wraps the shared `services::fs` module with MCP protocol adaptation and path security controls.

#### Scenario: List files in directory
- **WHEN** `file_list` tool is called with a valid path
- **AND** the path is in the `allowed_roots` configuration
- **THEN** the system SHALL return a list of files and directories
- **AND** each entry SHALL include `name` and `is_dir` properties

#### Scenario: Read file contents
- **WHEN** `file_read` tool is called with a valid file path
- **AND** the path is in the `allowed_roots` configuration
- **THEN** the system SHALL return the file contents as text
- **AND** the system SHALL handle encoding errors gracefully

#### Scenario: Write file contents
- **WHEN** `file_write` tool is called with path and content
- **AND** the path is in the `allowed_roots` configuration
- **AND** the user has confirmed the operation (if `requires_confirmation` is true)
- **THEN** the system SHALL write the content to the file
- **AND** the system SHALL return success status

#### Scenario: Reject path outside allowed roots
- **WHEN** any file tool is called with a path outside `allowed_roots`
- **THEN** the system SHALL return an error "Path not allowed"
- **AND** the system SHALL NOT perform any file operation

---

### Requirement: GitService (MCP Adapter for services::git)

The system SHALL provide a builtin Git MCP service that wraps the shared `services::git` module with MCP protocol adaptation and repository path security controls.

#### Scenario: Get repository status
- **WHEN** `git_status` tool is called with a valid repository path
- **AND** the path is in the `allowed_repos` configuration
- **THEN** the system SHALL return a list of changed files
- **AND** each entry SHALL include `path` and `status` properties

#### Scenario: Get commit history
- **WHEN** `git_log` tool is called with repository path and optional limit
- **THEN** the system SHALL return commit history
- **AND** each entry SHALL include `sha`, `message`, `author`, and `time`
- **AND** the default limit SHALL be 10 commits

#### Scenario: Get diff of changes
- **WHEN** `git_diff` tool is called with repository path
- **THEN** the system SHALL return the diff of changes
- **AND** the system SHALL support both staged and unstaged changes

#### Scenario: Execute git2 operations asynchronously
- **WHEN** any git tool is called
- **THEN** the system SHALL wrap blocking git2 calls with `tokio::task::spawn_blocking`
- **AND** the system SHALL NOT block the async runtime

---

### Requirement: SystemInfoService (MCP Adapter for services::system_info)

The system SHALL provide a builtin system information MCP service that wraps the shared `services::system_info` module with MCP protocol adaptation.

#### Scenario: Get system information
- **WHEN** `sys_info` tool is called
- **THEN** the system SHALL delegate to `SystemInfoProvider.get_info()`
- **AND** the system SHALL return system information as JSON

#### Scenario: Get active application
- **WHEN** `active_app` tool is called
- **THEN** the system SHALL delegate to `SystemInfoProvider.active_application()`
- **AND** the system SHALL return the frontmost application name

---

### Requirement: ShellService (Builtin Shell Command Service - Standalone)

The system SHALL provide a builtin shell command service with security controls. Unlike other services, ShellService does NOT use shared foundation modules due to its security-sensitive nature.

#### Scenario: Execute whitelisted command
- **WHEN** `shell_exec` tool is called with a command
- **AND** the command's program is in the `allowed_commands` whitelist (if configured)
- **AND** the user has confirmed the operation
- **THEN** the system SHALL execute the command
- **AND** the system SHALL return `exit_code`, `stdout`, and `stderr`

#### Scenario: Reject non-whitelisted command
- **WHEN** `shell_exec` tool is called with a command
- **AND** `allowed_commands` is configured and non-empty
- **AND** the command's program is NOT in the whitelist
- **THEN** the system SHALL return an error "Command not in whitelist"

#### Scenario: Timeout protection
- **WHEN** a shell command runs longer than `timeout_seconds`
- **THEN** the system SHALL terminate the command
- **AND** the system SHALL return an error "Command timed out"

#### Scenario: Shell service disabled by default
- **WHEN** `[mcp.builtin.shell]` is not explicitly configured
- **THEN** the Shell service SHALL be disabled
- **AND** `shell_exec` tool SHALL NOT be available

---

### Requirement: McpClient Service Registry

The system SHALL provide an MCP client that manages builtin services and external servers.

#### Scenario: Register builtin services
- **WHEN** `McpClient` is initialized with configuration
- **THEN** the system SHALL register all enabled builtin services
- **AND** builtin services SHALL be available immediately (no subprocess startup)

#### Scenario: Aggregate tools from all services
- **WHEN** `list_tools()` is called
- **THEN** the system SHALL return tools from all registered builtin services
- **AND** the system SHALL return tools from all connected external servers

#### Scenario: Route tool calls to correct service
- **WHEN** `call_tool(name, args)` is called
- **THEN** the system SHALL find the service that provides the tool
- **AND** the system SHALL execute the tool on that service
- **AND** the system SHALL return the result

#### Scenario: Handle tool not found
- **WHEN** `call_tool()` is called with an unknown tool name
- **THEN** the system SHALL return `AlephError::McpToolNotFound`

---

### Requirement: External Server Runtime Detection

The system SHALL detect runtime availability for external MCP servers and gracefully handle missing runtimes.

#### Scenario: Check Node.js availability
- **WHEN** an external server has `requires_runtime = "node"`
- **THEN** the system SHALL check if `node` is available in PATH
- **AND** the system SHALL skip the server if Node.js is not found
- **AND** the system SHALL log a warning message

#### Scenario: Check Python availability
- **WHEN** an external server has `requires_runtime = "python"`
- **THEN** the system SHALL check if `python3` is available in PATH
- **AND** the system SHALL skip the server if Python is not found

#### Scenario: Check Bun availability
- **WHEN** an external server has `requires_runtime = "bun"`
- **THEN** the system SHALL check if `bun` is available in PATH
- **AND** the system SHALL skip the server if Bun is not found

#### Scenario: No runtime requirement
- **WHEN** an external server has no `requires_runtime` field
- **THEN** the system SHALL attempt to start the server directly
- **AND** the system SHALL handle startup failure gracefully

---

### Requirement: Stdio Transport for External Servers

The system SHALL support Stdio transport for communicating with external MCP servers via subprocess.

#### Scenario: Spawn external server
- **WHEN** `StdioTransport::spawn(command, args, env)` is called
- **THEN** the system SHALL create a subprocess with piped stdin/stdout
- **AND** the system SHALL configure environment variables
- **AND** the process SHALL be terminated when transport is dropped

#### Scenario: Send JSON-RPC request
- **WHEN** `StdioTransport::send(request)` is called
- **THEN** the system SHALL serialize request to JSON
- **AND** the system SHALL write to subprocess stdin with newline delimiter
- **AND** the system SHALL read response from stdout
- **AND** the system SHALL return deserialized `JsonRpcResponse`

#### Scenario: Handle subprocess timeout
- **WHEN** a subprocess does not respond within timeout (default: 30 seconds)
- **THEN** the system SHALL return `AlephError::McpTimeout`
- **AND** the system SHALL NOT terminate the subprocess

---

### Requirement: MCP Capability Strategy Integration

The system SHALL integrate MCP as a capability using the existing `CapabilityStrategy` pattern.

#### Scenario: Execute MCP capability with builtin services
- **WHEN** `McpStrategy.execute(payload)` is called
- **AND** builtin services are registered
- **THEN** the system SHALL list available tools
- **AND** the system SHALL populate `payload.context.mcp_context.available_tools`

#### Scenario: Execute MCP capability without configuration
- **WHEN** `McpStrategy.execute(payload)` is called
- **AND** MCP is not configured
- **THEN** the system SHALL log debug message
- **AND** the system SHALL return payload unchanged

#### Scenario: MCP priority order
- **WHEN** multiple capabilities are configured
- **THEN** MCP SHALL execute after Memory (priority 0) and Search (priority 1)
- **AND** MCP priority SHALL be 2

---

### Requirement: MCP Context Formatting

The system SHALL format MCP context into LLM-readable prompt content.

#### Scenario: Format available tools as Markdown
- **WHEN** `PromptAssembler.format_mcp_context_markdown()` is called
- **AND** `McpContext.available_tools` is not empty
- **THEN** the output SHALL include tool names and descriptions
- **AND** the output SHALL include input schema for each tool

#### Scenario: Format tool results as Markdown
- **WHEN** `PromptAssembler.format_mcp_context_markdown()` is called
- **AND** `McpContext.tool_results` is not empty
- **THEN** the output SHALL include tool name and status
- **AND** the output SHALL include JSON-formatted result content

---

### Requirement: MCP Configuration

The system SHALL support TOML-based MCP configuration with builtin and external sections.

#### Scenario: Parse MCP enabled flag
- **WHEN** `config.toml` contains `[mcp]` section
- **THEN** the system SHALL parse `enabled` boolean (default: false)

#### Scenario: Parse builtin service configuration
- **WHEN** `[mcp.builtin.fs]` section exists
- **THEN** the system SHALL parse `enabled` boolean (default: true)
- **AND** the system SHALL parse `allowed_roots` array of paths
- **AND** the system SHALL expand `~` to home directory

#### Scenario: Parse Git service configuration
- **WHEN** `[mcp.builtin.git]` section exists
- **THEN** the system SHALL parse `enabled` boolean (default: true)
- **AND** the system SHALL parse `allowed_repos` array of paths

#### Scenario: Parse Shell service configuration
- **WHEN** `[mcp.builtin.shell]` section exists
- **THEN** the system SHALL parse `enabled` boolean (default: false)
- **AND** the system SHALL parse `timeout_seconds` (default: 30)
- **AND** the system SHALL parse `allowed_commands` array

#### Scenario: Parse external server configuration
- **WHEN** `[mcp.servers.<name>]` section exists
- **THEN** the system SHALL parse `transport` string (e.g., "stdio")
- **AND** the system SHALL parse `command` string
- **AND** the system SHALL parse `args` array (optional)
- **AND** the system SHALL parse `env` table (optional)
- **AND** the system SHALL parse `requires_runtime` string (optional)

---

### Requirement: MCP Permission Control

The system SHALL enforce permission control for MCP tool calls.

#### Scenario: Tools requiring confirmation
- **WHEN** a tool is listed in `dangerous_tools` configuration
- **AND** `require_confirmation` is true
- **THEN** the system SHALL trigger `ToolConfirmationRequired` callback
- **AND** the system SHALL wait for user response before executing

#### Scenario: User denies tool execution
- **WHEN** user denies the tool confirmation dialog
- **THEN** the system SHALL return an error "Tool execution denied by user"
- **AND** the system SHALL NOT execute the tool

#### Scenario: Default dangerous tools
- **WHEN** no `dangerous_tools` configuration is provided
- **THEN** the system SHALL use default list:
  - `file_write`, `file_delete`
  - `shell_exec`
  - `git_commit`, `git_push`, `git_reset`

---

### Requirement: Zero External Dependency Guarantee

The system SHALL guarantee zero external dependencies for builtin MCP services.

#### Scenario: Builtin services work without Node.js
- **WHEN** Node.js is not installed on the system
- **THEN** all builtin services (fs, git, shell) SHALL function correctly
- **AND** the user SHALL NOT see "npm not found" or similar errors

#### Scenario: Git operations without git CLI
- **WHEN** git CLI is not installed on the system
- **THEN** the GitService SHALL function correctly using git2-rs library
- **AND** git operations SHALL NOT spawn external processes

#### Scenario: File operations without external tools
- **WHEN** FsService performs file operations
- **THEN** the system SHALL use Rust standard library and tokio
- **AND** the system SHALL NOT spawn external processes like `cat`, `ls`, etc.
