# Tasks: add-cowork-file-operations

## 1. FileOps Data Types

- [x] 1.1 Extend `FileOp` enum with full operation set (Read, Write, Move, Copy, Delete, Search, List)
- [x] 1.2 Define `FileOpResult` struct with content, metadata, affected_paths
- [x] 1.3 Define `FilePermission` enum (ReadOnly, ReadWrite, Full) - Simplified to permission checker
- [x] 1.4 Define `FileOpError` error types (PermissionDenied, NotFound, SizeLimitExceeded)
- [x] 1.5 Add file operation parameters to Task.parameters schema

## 2. Permission System

- [x] 2.1 Create `permission.rs` module in executor/
- [x] 2.2 Implement `PathPermissionChecker` with allowed/denied paths
- [x] 2.3 Add glob pattern support for path matching
- [x] 2.4 Implement path canonicalization (resolve symlinks, ~, ..)
- [x] 2.5 Add default denied paths (~/.ssh, ~/.gnupg, etc.)
- [x] 2.6 Write unit tests for permission checking

## 3. FileOps Executor

- [x] 3.1 Create `executor/file_ops.rs` module
- [x] 3.2 Implement `FileOpsExecutor` struct with permission checker
- [x] 3.3 Implement `execute_read()` - read file with size limit check
- [x] 3.4 Implement `execute_write()` - write with parent dir creation
- [x] 3.5 Implement `execute_move()` - atomic move with rollback
- [x] 3.6 Implement `execute_copy()` - copy with progress reporting
- [x] 3.7 Implement `execute_delete()` - soft delete (trash) option
- [x] 3.8 Implement `execute_search()` - glob/regex file search
- [x] 3.9 Implement `execute_list()` - directory listing with metadata
- [x] 3.10 Write unit tests for each operation

## 4. Batch Operations

- [x] 4.1 Implement `BatchFileOp` for multiple operations (BatchMove implemented)
- [ ] 4.2 Add parallel IO with configurable concurrency
- [x] 4.3 Implement progress tracking for batch operations
- [ ] 4.4 Add atomic batch mode (all-or-nothing)
- [ ] 4.5 Implement rollback for failed batch operations
- [x] 4.6 Write tests for batch operations

## 5. Configuration

- [x] 5.1 Add `FileOpsConfigToml` struct to config/types/cowork.rs
- [x] 5.2 Add `allowed_paths` and `denied_paths` fields
- [x] 5.3 Add `max_file_size` field with human-readable parsing
- [x] 5.4 Add `require_confirmation_for_write/delete` fields
- [x] 5.5 Implement config validation
- [ ] 5.6 Document configuration options

## 6. Integration

- [x] 6.1 Register FileOpsExecutor in ExecutorRegistry
- [x] 6.2 Update CoworkEngine to load file_ops config
- [ ] 6.3 Add file operation confirmation to HaloState
- [x] 6.4 Update UniFFI bindings for new types (not needed - existing bindings sufficient)
- [ ] 6.5 Test end-to-end file operation task

## 7. Swift UI

- [x] 7.1 Add FileOps section to CoworkSettingsView
- [x] 7.2 Create AllowedPathsEditor component
- [x] 7.3 Create DeniedPathsEditor component
- [x] 7.4 Add file size limit picker
- [x] 7.5 Add confirmation toggles for write/delete
- [x] 7.6 Add localization strings

## 8. Security Review

- [x] 8.1 Review path traversal prevention (implemented in permission.rs)
- [x] 8.2 Review symlink handling (canonicalize resolves symlinks)
- [ ] 8.3 Review race condition mitigation
- [x] 8.4 Review error message information leakage (errors don't reveal denied paths list)
- [ ] 8.5 Document security model

## 9. Testing & Documentation

- [x] 9.1 Write integration tests for FileOpsExecutor
- [x] 9.2 Test permission denial scenarios
- [ ] 9.3 Test batch operation rollback
- [x] 9.4 Update docs/COWORK.md with FileOps section
- [ ] 9.5 Add example usage scenarios
- [x] 9.6 Run cargo clippy and fix warnings

## Completion Checklist

- [ ] All tasks in sections 1-9 completed
- [x] All tests passing (66 tests)
- [ ] Security review completed
- [ ] Documentation updated
- [ ] Ready for Phase 3 (Code Executor)
