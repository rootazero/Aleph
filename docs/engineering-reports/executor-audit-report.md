# Logic Review Report
**Module**: src/executor
**Scope**: Full module static audit (16 files, ~3800 lines)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Warning] Silent schema serialization failures via unwrap_or_default()
- **Location**: `builtin_registry/builder/core_tools.rs` (22 occurrences), `builtin_registry/builder/optional_tools.rs` (25 occurrences), `builtin_registry/builder/constructor.rs` (6 occurrences)
- **Risk**: If `serde_json::to_value(schema_for!(...))` fails, the tool registers with a null/empty `parameters_schema`. The LLM then receives a tool definition without parameter hints, leading to malformed tool calls or silent degradation of tool discovery.
- **Current impact**: medium
- **Suggestion**: Replace `unwrap_or_default()` with explicit error handling. At minimum log a warning when schema serialization fails so operators know a tool is degraded.

### [Warning] Truncating cast from u128 to u64 for execution time
- **Location**: `types.rs:86`, `types.rs:183`, `types.rs:291`
- **Risk**: `Duration::as_millis()` returns `u128`. The `as u64` cast silently truncates values > `u64::MAX` (~58 million years). While practically impossible for normal execution times, this pattern sets a bad precedent and could mask bugs in pathological cases (e.g. clock tampering, `Instant` wrap-around on exotic platforms).
- **Current impact**: low
- **Suggestion**: Use `u64::try_from(time.as_millis()).unwrap_or(u64::MAX)` to make the truncation explicit and bounded.

### [Warning] Constructor panic via expect() on I/O paths
- **Location**: `builtin_registry/builder/constructor.rs:397`, `constructor.rs:596`, `constructor.rs:1243`
- **Risk**: `SessionManager::with_defaults().expect(...)` and `ClawHubTool::new().expect(...)` will panic if the underlying I/O or network operation fails. This turns a recoverable initialization error into a process crash.
- **Current impact**: medium
- **Suggestion**: Degrade gracefully — return a `Result` from the constructor or skip the optional component and log a warning instead of panicking.

### [Warning] unreachable!() arm in nested match
- **Location**: `builtin_registry/registry.rs:976`
- **Risk**: The outer `match` restricts `tool_name` to `"agent_create" | "agent_list" | "agent_delete"`, so the inner `_ => unreachable!()` is logically safe today. However, if the outer match is ever extended (e.g. adding a new agent tool) without updating the inner match, this becomes a production panic.
- **Current impact**: low
- **Suggestion**: Replace with `return Box::pin(async move { Err(AlephError::tool(format!("Agent tool '{}' not yet wired", tool_name))) })` to make the code robust to future additions.

### [Warning] Direct tokio::sync::RwLock import bypasses sync_primitives abstraction
- **Location**: `cache_store.rs:11`
- **Risk**: The project provides `crate::sync_primitives::AsyncRwLock` as the canonical async RwLock type (enables loom instrumentation if needed). Importing `tokio::sync::RwLock` directly breaks the abstraction and prevents future concurrency testing of this module via loom.
- **Current impact**: low
- **Suggestion**: Replace `use tokio::sync::RwLock;` with `use crate::sync_primitives::AsyncRwLock as RwLock;`.

### [Warning] Empty PathBuf fallback when home_dir is unavailable
- **Location**: `builtin_registry/builder/constructor.rs:213`, `constructor.rs:231`, `constructor.rs:979`
- **Risk**: `dirs::home_dir().unwrap_or_default()` returns an empty `PathBuf` when the user's home directory cannot be determined. Subsequent path joins (`join(".aleph").join("memory")`) produce a relative path `".aleph/memory/note"` that resolves to the current working directory, which is unpredictable and could write user data to an unintended location (e.g. the project root, `/tmp`, etc.).
- **Current impact**: medium
- **Suggestion**: Use a more robust fallback such as `std::env::temp_dir()` or return an error when home_dir is unavailable.

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 6 |
| Suggested Test | 0 |

## Cross-Module Findings

None — this audit focused exclusively on `src/executor`.
