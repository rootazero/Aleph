# Cowork Code Execution

## ADDED Requirements

### Requirement: Code Execution Runtime Support

The system SHALL support executing code in multiple runtimes including Shell (bash/zsh), Python, and Node.js.

#### Scenario: Shell command execution
- **WHEN** a task specifies runtime "shell" with command "echo hello"
- **THEN** the system executes the command using the default shell
- **AND** captures stdout containing "hello"

#### Scenario: Python script execution
- **WHEN** a task specifies runtime "python" with code "print(2+2)"
- **THEN** the system executes using python3
- **AND** captures stdout containing "4"

#### Scenario: Node.js script execution
- **WHEN** a task specifies runtime "nodejs" with code "console.log(JSON.stringify({a:1}))"
- **THEN** the system executes using node
- **AND** captures stdout containing '{"a":1}'

#### Scenario: Runtime not available
- **WHEN** a task specifies a runtime that is not installed
- **THEN** the system returns RuntimeNotFound error
- **AND** includes the runtime name in error message

### Requirement: Execution Sandboxing

The system SHALL execute code in a sandboxed environment when sandbox mode is enabled.

#### Scenario: Sandbox restricts file access
- **WHEN** sandbox is enabled and code attempts to read /etc/passwd
- **AND** /etc/passwd is not in allowed_paths
- **THEN** the read operation fails with permission error

#### Scenario: Sandbox restricts network access
- **WHEN** sandbox is enabled and allow_network is false
- **AND** code attempts to make HTTP request
- **THEN** the network operation fails

#### Scenario: Sandbox allows configured paths
- **WHEN** sandbox is enabled and code reads from allowed_paths
- **THEN** the read operation succeeds

### Requirement: Execution Timeout

The system SHALL enforce a configurable timeout for code execution.

#### Scenario: Execution completes within timeout
- **WHEN** code executes in less than timeout_seconds
- **THEN** the result includes exit code, stdout, and stderr

#### Scenario: Execution exceeds timeout
- **WHEN** code runs longer than timeout_seconds
- **THEN** the process is terminated
- **AND** Timeout error is returned
- **AND** partial output is captured if available

### Requirement: Dangerous Command Blocking

The system SHALL block execution of commands matching the blocked_commands list.

#### Scenario: Block rm -rf command
- **WHEN** command contains "rm -rf /"
- **THEN** execution is blocked before spawning process
- **AND** Blocked error is returned with reason

#### Scenario: Block sudo command
- **WHEN** command contains "sudo"
- **THEN** execution is blocked
- **AND** Blocked error is returned

#### Scenario: Allow safe commands
- **WHEN** command does not match any blocked patterns
- **AND** command passes other validation
- **THEN** execution proceeds normally

### Requirement: Output Capture

The system SHALL capture stdout and stderr from executed code with size limits.

#### Scenario: Capture stdout and stderr separately
- **WHEN** code writes to both stdout and stderr
- **THEN** both streams are captured separately in result

#### Scenario: Truncate large output
- **WHEN** stdout exceeds 10MB
- **THEN** output is truncated at limit
- **AND** truncation marker is appended

#### Scenario: Handle non-UTF8 output
- **WHEN** output contains non-UTF8 bytes
- **THEN** output is decoded with replacement characters
- **AND** encoding warning is included

### Requirement: Configuration

The system SHALL support configuring code execution behavior via config.toml.

#### Scenario: Code execution disabled by default
- **WHEN** config does not explicitly enable code_exec
- **THEN** CodeExecution tasks return error "Code execution is disabled"

#### Scenario: Configure allowed runtimes
- **WHEN** allowed_runtimes is set to ["shell", "python"]
- **AND** task requests nodejs runtime
- **THEN** execution fails with "Runtime not allowed" error

#### Scenario: Configure environment variables
- **WHEN** pass_env includes "MY_VAR"
- **AND** MY_VAR is set in parent environment
- **THEN** MY_VAR is available to executed code

### Requirement: Integration with Permission System

The system SHALL integrate code execution with the existing file permission system.

#### Scenario: Inherit file permissions
- **WHEN** code execution accesses file paths
- **THEN** paths are validated against FileOps allowed_paths

#### Scenario: Working directory validation
- **WHEN** working_directory is configured
- **THEN** working_directory must be in allowed_paths
- **AND** execution runs in that directory
