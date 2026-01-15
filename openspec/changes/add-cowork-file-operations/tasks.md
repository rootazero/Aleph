# Tasks: add-cowork-file-operations

## 1. FileOps Data Types

- [ ] 1.1 Extend `FileOp` enum with full operation set (Read, Write, Move, Copy, Delete, Search, List)
- [ ] 1.2 Define `FileOpResult` struct with content, metadata, affected_paths
- [ ] 1.3 Define `FilePermission` enum (ReadOnly, ReadWrite, Full)
- [ ] 1.4 Define `FileOpError` error types (PermissionDenied, NotFound, SizeLimitExceeded)
- [ ] 1.5 Add file operation parameters to Task.parameters schema

## 2. Permission System

- [ ] 2.1 Create `permission/mod.rs` module in cowork
- [ ] 2.2 Implement `PathPermissionChecker` with allowed/denied paths
- [ ] 2.3 Add glob pattern support for path matching
- [ ] 2.4 Implement path canonicalization (resolve symlinks, ~, ..)
- [ ] 2.5 Add default denied paths (~/.ssh, ~/.gnupg, etc.)
- [ ] 2.6 Write unit tests for permission checking

## 3. FileOps Executor

- [ ] 3.1 Create `executor/file_ops.rs` module
- [ ] 3.2 Implement `FileOpsExecutor` struct with permission checker
- [ ] 3.3 Implement `execute_read()` - read file with size limit check
- [ ] 3.4 Implement `execute_write()` - write with parent dir creation
- [ ] 3.5 Implement `execute_move()` - atomic move with rollback
- [ ] 3.6 Implement `execute_copy()` - copy with progress reporting
- [ ] 3.7 Implement `execute_delete()` - soft delete (trash) option
- [ ] 3.8 Implement `execute_search()` - glob/regex file search
- [ ] 3.9 Implement `execute_list()` - directory listing with metadata
- [ ] 3.10 Write unit tests for each operation

## 4. Batch Operations

- [ ] 4.1 Implement `BatchFileOp` for multiple operations
- [ ] 4.2 Add parallel IO with configurable concurrency
- [ ] 4.3 Implement progress tracking for batch operations
- [ ] 4.4 Add atomic batch mode (all-or-nothing)
- [ ] 4.5 Implement rollback for failed batch operations
- [ ] 4.6 Write tests for batch operations

## 5. Configuration

- [ ] 5.1 Add `FileOpsConfigToml` struct to config/types/cowork.rs
- [ ] 5.2 Add `allowed_paths` and `denied_paths` fields
- [ ] 5.3 Add `max_file_size` field with human-readable parsing
- [ ] 5.4 Add `require_confirmation_for_write/delete` fields
- [ ] 5.5 Implement config validation
- [ ] 5.6 Document configuration options

## 6. Integration

- [ ] 6.1 Register FileOpsExecutor in ExecutorRegistry
- [ ] 6.2 Update CoworkEngine to load file_ops config
- [ ] 6.3 Add file operation confirmation to HaloState
- [ ] 6.4 Update UniFFI bindings for new types
- [ ] 6.5 Test end-to-end file operation task

## 7. Swift UI

- [ ] 7.1 Add FileOps section to CoworkSettingsView
- [ ] 7.2 Create AllowedPathsEditor component
- [ ] 7.3 Create DeniedPathsEditor component
- [ ] 7.4 Add file size limit picker
- [ ] 7.5 Add confirmation toggles for write/delete
- [ ] 7.6 Add localization strings

## 8. Security Review

- [ ] 8.1 Review path traversal prevention
- [ ] 8.2 Review symlink handling
- [ ] 8.3 Review race condition mitigation
- [ ] 8.4 Review error message information leakage
- [ ] 8.5 Document security model

## 9. Testing & Documentation

- [ ] 9.1 Write integration tests for FileOpsExecutor
- [ ] 9.2 Test permission denial scenarios
- [ ] 9.3 Test batch operation rollback
- [ ] 9.4 Update docs/COWORK.md with FileOps section
- [ ] 9.5 Add example usage scenarios
- [ ] 9.6 Run cargo clippy and fix warnings

## Completion Checklist

- [ ] All tasks in sections 1-9 completed
- [ ] All tests passing
- [ ] Security review completed
- [ ] Documentation updated
- [ ] Ready for Phase 3 (Code Executor)
