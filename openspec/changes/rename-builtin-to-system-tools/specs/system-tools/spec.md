# System Tools Capability

## ADDED Requirements

### Requirement: Two-Tier Tool Architecture

The system SHALL implement a two-tier tool architecture separating System Built-ins (Tier 1) from MCP Extensions (Tier 2).

#### Scenario: System tools at top-level
- **Given** the command palette is open
- **When** the user types `/`
- **Then** system tools (`/fs`, `/git`, `/sys`, `/shell`) appear at the top level, NOT under `/mcp/`

#### Scenario: MCP extensions under namespace
- **Given** the command palette is open
- **When** the user types `/mcp`
- **Then** only user-installed external MCP servers appear (e.g., `/mcp/linear`, `/mcp/postgres`)

### Requirement: System Tools Module Location

The system SHALL organize system tools under `services/tools/` module, separate from the `mcp/` module which handles external servers only.

#### Scenario: Code organization
- **Given** a developer navigating the codebase
- **When** looking for system tool implementations (fs, git, shell, sys)
- **Then** they find them in `services/tools/` (NOT in `mcp/builtin/`)

#### Scenario: MCP module purity
- **Given** the `mcp/` module
- **When** examining its contents
- **Then** it only contains external MCP server logic (JSON-RPC, transport, client)

### Requirement: System Tools Configuration

The system SHALL provide a dedicated `[tools]` config section for system tools, separate from `[mcp]` which is for external MCP servers only.

#### Scenario: Config section separation
- **Given** a user editing config.toml
- **When** configuring system tools (fs, git, shell, sys)
- **Then** they use the `[tools]` section

#### Scenario: MCP config for extensions only
- **Given** a user editing config.toml
- **When** configuring external MCP servers
- **Then** they use the `[mcp]` section with `[[mcp.servers]]` entries

### Requirement: Config Migration

The system SHALL auto-migrate legacy `[mcp.builtin]` config to `[tools]` section with a deprecation warning.

#### Scenario: Legacy config detection
- **Given** a config.toml with `[mcp.builtin]` section
- **When** Aleph loads the config
- **Then** it migrates settings to `[tools]` and logs a deprecation warning

#### Scenario: New config format
- **Given** a config.toml with `[tools]` section
- **When** Aleph loads the config
- **Then** it uses the settings directly without migration

### Requirement: System Tools UI Labels

The UI SHALL display "System Tools" for built-in tools and "MCP Extensions" for external servers.

#### Scenario: Settings view sections
- **Given** the Settings view for tools/MCP
- **When** displaying the configuration UI
- **Then** there are two distinct sections: "System Tools" and "MCP Extensions"

#### Scenario: Command palette grouping
- **Given** the command palette
- **When** showing available commands
- **Then** system tools are visually grouped separately from MCP extensions

## MODIFIED Requirements

### Requirement: Tool Trigger Commands

The system SHALL use short, shell-like trigger commands for system tools at the top level.

#### Scenario: File system command
- **Given** the user wants to read a file
- **When** they type `/fs/read`
- **Then** the file system read tool is invoked (NOT `/mcp/fs/read`)

#### Scenario: Git command
- **Given** the user wants to check git status
- **When** they type `/git/status`
- **Then** the git status tool is invoked (NOT `/mcp/git/status`)

#### Scenario: System info command
- **Given** the user wants system information
- **When** they type `/sys/info`
- **Then** the system info tool is invoked (NOT `/mcp/system/info`)

### Requirement: Legacy Command Aliases

The system SHALL support legacy command paths with deprecation warnings during a transition period.

#### Scenario: Legacy fs command
- **Given** a user types `/mcp/fs/read`
- **When** the command is processed
- **Then** it routes to `/fs/read` and logs a deprecation warning

#### Scenario: Legacy system command
- **Given** a user types `/mcp/system/info`
- **When** the command is processed
- **Then** it routes to `/sys/info` and logs a deprecation warning
