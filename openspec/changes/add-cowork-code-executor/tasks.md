# Tasks: add-cowork-code-executor

## 1. Data Types

- [ ] 1.1 Define `CodeExecRuntime` enum (Shell, Python, NodeJs)
- [ ] 1.2 Define `CodeExecResult` struct (exit_code, stdout, stderr, duration)
- [ ] 1.3 Define `CodeExecError` error types (RuntimeNotFound, Timeout, Blocked, SandboxError)
- [ ] 1.4 Add `CodeExecution` variant to `TaskType` enum
- [ ] 1.5 Define `RuntimeInfo` struct for runtime detection

## 2. Runtime Detection

- [ ] 2.1 Create `runtime.rs` module in executor/
- [ ] 2.2 Implement `RuntimeManager` for detecting available runtimes
- [ ] 2.3 Add `which` command wrapper for path detection
- [ ] 2.4 Implement version detection for each runtime
- [ ] 2.5 Add runtime availability caching
- [ ] 2.6 Write unit tests for runtime detection

## 3. Command Validation

- [ ] 3.1 Create `command_check.rs` module
- [ ] 3.2 Implement dangerous command blocklist
- [ ] 3.3 Add regex patterns for dangerous commands
- [ ] 3.4 Implement path extraction from commands
- [ ] 3.5 Integrate with PathPermissionChecker
- [ ] 3.6 Write unit tests for command validation

## 4. Sandbox Implementation

- [ ] 4.1 Create `sandbox.rs` module
- [ ] 4.2 Define sandbox profile template for macOS
- [ ] 4.3 Implement `SandboxConfig` with configurable permissions
- [ ] 4.4 Implement `generate_sandbox_profile()` function
- [ ] 4.5 Add file path rules based on allowed_paths
- [ ] 4.6 Add network access rules (allow/deny)
- [ ] 4.7 Write integration tests for sandbox profiles

## 5. Code Executor

- [ ] 5.1 Create `executor/code_exec.rs` module
- [ ] 5.2 Implement `CodeExecutor` struct
- [ ] 5.3 Implement `TaskExecutor` trait for CodeExecutor
- [ ] 5.4 Implement `execute_shell()` method
- [ ] 5.5 Implement `execute_python()` method
- [ ] 5.6 Implement `execute_node()` method
- [ ] 5.7 Add timeout handling with tokio::time::timeout
- [ ] 5.8 Implement output capture with size limits
- [ ] 5.9 Implement process cleanup on timeout/cancel
- [ ] 5.10 Write unit tests for each execution method

## 6. Output Handling

- [ ] 6.1 Create `OutputCapture` struct for stdout/stderr
- [ ] 6.2 Implement streaming output capture
- [ ] 6.3 Add configurable size limits (10MB stdout, 1MB stderr)
- [ ] 6.4 Implement truncation with warning marker
- [ ] 6.5 Add encoding detection (UTF-8 with fallback)
- [ ] 6.6 Write tests for output handling

## 7. Configuration

- [ ] 7.1 Add `CodeExecConfigToml` struct to config/types/cowork.rs
- [ ] 7.2 Add `enabled` field (default: false)
- [ ] 7.3 Add `default_runtime` field
- [ ] 7.4 Add `timeout_seconds` field
- [ ] 7.5 Add `sandbox_enabled` field
- [ ] 7.6 Add `allowed_runtimes` field
- [ ] 7.7 Add `allow_network` field
- [ ] 7.8 Add `working_directory` field
- [ ] 7.9 Add `pass_env` field for environment variables
- [ ] 7.10 Add `blocked_commands` field
- [ ] 7.11 Implement config validation
- [ ] 7.12 Write tests for configuration parsing

## 8. Integration

- [ ] 8.1 Register CodeExecutor in ExecutorRegistry
- [ ] 8.2 Update CoworkEngine to load code_exec config
- [ ] 8.3 Add code execution preview to HaloState
- [ ] 8.4 Update UniFFI bindings for new types
- [ ] 8.5 Test end-to-end code execution task

## 9. Swift UI

- [ ] 9.1 Add CodeExec section to CoworkSettingsView
- [ ] 9.2 Create runtime selector component
- [ ] 9.3 Add timeout picker
- [ ] 9.4 Add sandbox toggle with explanation
- [ ] 9.5 Add network access toggle
- [ ] 9.6 Add blocked commands editor
- [ ] 9.7 Add localization strings

## 10. Security Review

- [ ] 10.1 Review command injection prevention
- [ ] 10.2 Review sandbox escape vectors
- [ ] 10.3 Review resource limit effectiveness
- [ ] 10.4 Review environment variable leakage
- [ ] 10.5 Document security model
- [ ] 10.6 Add security warnings to UI

## 11. Testing & Documentation

- [ ] 11.1 Write integration tests for CodeExecutor
- [ ] 11.2 Test sandbox isolation
- [ ] 11.3 Test timeout behavior
- [ ] 11.4 Test blocked command detection
- [ ] 11.5 Update docs/COWORK.md with CodeExec section
- [ ] 11.6 Add example usage scenarios
- [ ] 11.7 Run cargo clippy and fix warnings

## Completion Checklist

- [ ] All tasks in sections 1-11 completed
- [ ] All tests passing
- [ ] Security review completed
- [ ] Documentation updated
- [ ] Ready for Phase 4
