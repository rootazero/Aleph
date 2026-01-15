# Tasks: add-cowork-code-executor

## 1. Data Types

- [x] 1.1 Define `CodeExecRuntime` enum (Shell, Python, NodeJs) - Used existing Language enum
- [x] 1.2 Define `CodeExecResult` struct (exit_code, stdout, stderr, duration)
- [x] 1.3 Define `CodeExecError` error types (RuntimeNotFound, Timeout, Blocked, SandboxError)
- [x] 1.4 Add `CodeExecution` variant to `TaskType` enum - Already existed
- [x] 1.5 Define `RuntimeInfo` struct for runtime detection

## 2. Runtime Detection

- [x] 2.1 Create `runtime.rs` module in executor/ - Integrated into code_exec.rs
- [x] 2.2 Implement `RuntimeManager` for detecting available runtimes - RuntimeInfo::detect()
- [x] 2.3 Add `which` command wrapper for path detection
- [x] 2.4 Implement version detection for each runtime
- [x] 2.5 Add runtime availability caching
- [x] 2.6 Write unit tests for runtime detection

## 3. Command Validation

- [x] 3.1 Create `command_check.rs` module - Integrated into code_exec.rs as CommandChecker
- [x] 3.2 Implement dangerous command blocklist
- [x] 3.3 Add regex patterns for dangerous commands
- [x] 3.4 Implement path extraction from commands
- [x] 3.5 Integrate with PathPermissionChecker
- [x] 3.6 Write unit tests for command validation

## 4. Sandbox Implementation

- [x] 4.1 Create `sandbox.rs` module - Integrated into code_exec.rs as SandboxConfig
- [x] 4.2 Define sandbox profile template for macOS
- [x] 4.3 Implement `SandboxConfig` with configurable permissions
- [x] 4.4 Implement `generate_sandbox_profile()` function
- [x] 4.5 Add file path rules based on allowed_paths
- [x] 4.6 Add network access rules (allow/deny)
- [ ] 4.7 Write integration tests for sandbox profiles

## 5. Code Executor

- [x] 5.1 Create `executor/code_exec.rs` module
- [x] 5.2 Implement `CodeExecutor` struct
- [x] 5.3 Implement `TaskExecutor` trait for CodeExecutor
- [x] 5.4 Implement `execute_shell()` method - execute_command()
- [x] 5.5 Implement `execute_python()` method - execute_script()
- [x] 5.6 Implement `execute_node()` method - execute_script()
- [x] 5.7 Add timeout handling with tokio::time::timeout
- [x] 5.8 Implement output capture with size limits
- [x] 5.9 Implement process cleanup on timeout/cancel
- [x] 5.10 Write unit tests for each execution method

## 6. Output Handling

- [x] 6.1 Create `OutputCapture` struct for stdout/stderr - Inline in run_process()
- [x] 6.2 Implement streaming output capture
- [x] 6.3 Add configurable size limits (10MB stdout, 1MB stderr)
- [x] 6.4 Implement truncation with warning marker
- [x] 6.5 Add encoding detection (UTF-8 with fallback)
- [x] 6.6 Write tests for output handling

## 7. Configuration

- [x] 7.1 Add `CodeExecConfigToml` struct to config/types/cowork.rs
- [x] 7.2 Add `enabled` field (default: false)
- [x] 7.3 Add `default_runtime` field
- [x] 7.4 Add `timeout_seconds` field
- [x] 7.5 Add `sandbox_enabled` field
- [x] 7.6 Add `allowed_runtimes` field
- [x] 7.7 Add `allow_network` field
- [x] 7.8 Add `working_directory` field
- [x] 7.9 Add `pass_env` field for environment variables
- [x] 7.10 Add `blocked_commands` field
- [x] 7.11 Implement config validation
- [ ] 7.12 Write tests for configuration parsing

## 8. Integration

- [x] 8.1 Register CodeExecutor in ExecutorRegistry
- [x] 8.2 Update CoworkEngine to load code_exec config
- [ ] 8.3 Add code execution preview to HaloState
- [x] 8.4 Update UniFFI bindings for new types
- [ ] 8.5 Test end-to-end code execution task

## 9. Swift UI

- [x] 9.1 Add CodeExec section to CoworkSettingsView
- [x] 9.2 Create runtime selector component
- [x] 9.3 Add timeout picker
- [x] 9.4 Add sandbox toggle with explanation
- [x] 9.5 Add network access toggle
- [ ] 9.6 Add blocked commands editor
- [x] 9.7 Add localization strings

## 10. Security Review

- [x] 10.1 Review command injection prevention
- [ ] 10.2 Review sandbox escape vectors
- [x] 10.3 Review resource limit effectiveness
- [x] 10.4 Review environment variable leakage
- [ ] 10.5 Document security model
- [x] 10.6 Add security warnings to UI

## 11. Testing & Documentation

- [x] 11.1 Write integration tests for CodeExecutor
- [x] 11.2 Test sandbox isolation
- [x] 11.3 Test timeout behavior
- [x] 11.4 Test blocked command detection
- [ ] 11.5 Update docs/COWORK.md with CodeExec section
- [ ] 11.6 Add example usage scenarios
- [ ] 11.7 Run cargo clippy and fix warnings

## Completion Checklist

- [ ] All tasks in sections 1-11 completed
- [x] All tests passing (73 tests)
- [ ] Security review completed
- [ ] Documentation updated
- [ ] Ready for Phase 4
