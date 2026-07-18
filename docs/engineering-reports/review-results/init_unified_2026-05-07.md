# Logic Review Report
**Module**: init_unified
**Scope**: Full static review of src/init_unified/ (coordinator.rs, mod.rs, error.rs)
**Date**: 2026-05-07
**Mode**: strict

## Findings

### [Critical] Database rollback leaves SQLite WAL artifacts
- **Location**: `coordinator.rs:177-182`
- **Trigger condition**: Initialization fails after Database phase when SQLite is in WAL mode
- **Expected behavior**: Rollback should remove all database-related files
- **Actual behavior**: Only `memory.db` is removed, leaving `memory.db-wal` and `memory.db-shm` behind
- **Suggested fix**: Also remove `.db-wal` and `.db-shm` counterparts after removing the main DB file

### [Warning] Rollback silently ignores cleanup failures
- **Location**: `coordinator.rs:155-200`
- **Risk**: Partial rollback — caller believes cleanup succeeded when some files/directories remain
- **Current impact**: medium
- **Suggestion**: Collect all rollback errors into a single error report and return it, so callers can decide whether to retry or alert

### [Warning] spawn_blocking panic message lacks context
- **Location**: `coordinator.rs:293-295`
- **Risk**: If `migrate_from_legacy` panics, the panic payload (e.g., a custom error message) is lost; only "task panicked" is reported
- **Current impact**: low
- **Suggestion**: Check `e.is_panic()` and attempt to extract the panic payload for better diagnostics

### [Warning] Created directories use default OS permissions
- **Location**: `coordinator.rs:206-225`
- **Risk**: Config directory may be created with overly permissive permissions (e.g., 755 on Unix), allowing other users to read/write sensitive configuration
- **Current impact**: medium
- **Suggestion**: Set directory permissions to 0o700 (user-only) after creation on Unix systems

### [Warning] install_skills calls create_dir_all redundantly
- **Location**: `coordinator.rs:316-321`
- **Risk**: `skills` directory is already created in `create_directories` phase; redundant operation wastes I/O and could mask permission issues
- **Current impact**: low
- **Suggestion**: Remove the redundant `create_dir_all` in `install_skills` since the directory is guaranteed to exist after phase 1

## Summary
| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 4 |
| Suggested Test | 0 |
