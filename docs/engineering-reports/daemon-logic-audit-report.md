# Logic Review Report — Daemon Module

**Module**: `src/daemon`
**Scope**: Full static audit of all 41 Rust files under `src/daemon/`
**Date**: 2026-05-06
**Mode**: strict

---

## Critical Findings

### [Critical] `tokio::select!` with single branch will panic when event bus closes
- **Location**: `src/daemon/dispatcher/mod.rs:68-100`
- **Trigger condition**: When all `broadcast::Sender` instances are dropped, `rx.recv()` returns `Err(RecvError)`. The pattern `Ok(event)` fails to match, disabling the only branch. Per tokio docs, when all branches are disabled, `select!` panics at runtime.
- **Expected behavior**: Dispatcher should gracefully exit its loop when the event bus shuts down.
- **Actual behavior**: Runtime panic due to `select!` with all branches disabled.
- **Suggested fix**: Replace `tokio::select!` with a plain `loop { match rx.recv().await { Ok(e) => ..., Err(_) => break } }`.

### [Critical] Default paths use `~` literal which is not expanded by the OS
- **Location**: `src/daemon/types.rs:29,35,41`
- **Trigger condition**: When `dirs::home_dir()` returns `None` (e.g., in restricted environments, containers, or when `$HOME` is unset).
- **Expected behavior**: Fallback paths should be valid absolute paths that the OS can resolve.
- **Actual behavior**: Paths like `"~/.aleph/daemon.sock"` are passed literally to the OS. The `~` character is NOT expanded by the kernel or standard library; only shells expand it. This causes file operations to fail or create directories literally named `~`.
- **Suggested fix**: Use absolute fallback paths (`/tmp/.aleph/...`) instead of `~` literals.

---

## Warning Findings

### [Warning] Sync primitives import rule violation — `tokio::sync::RwLock` used instead of `crate::sync_primitives`
- **Location**: 
  - `src/daemon/resource_governor.rs:4`
  - `src/daemon/worldmodel/mod.rs:19`
  - `src/daemon/dispatcher/mod.rs:24`
- **Risk**: Violates Aleph invariant #8 (Sync Primitives Import Rule). If loom testing is introduced for daemon module, these async RwLock types won't be instrumented, leaving concurrency bugs undetected.
- **Current impact**: Medium — the code correctly avoids holding guards across await points, so no immediate deadlock risk. But it breaks the project's uniform sync primitive abstraction.
- **Suggestion**: Either (a) extend `sync_primitives` to re-export `tokio::sync::RwLock` under a feature flag, or (b) refactor to use `std::sync::RwLock` from `crate::sync_primitives` (safe here since guards never cross await).

### [Warning] TOCTOU race in YAML policy loader
- **Location**: `src/daemon/dispatcher/yaml_policy/loader.rs:18-24`
- **Risk**: File may be deleted or modified between `path.exists()` check and `fs::read_to_string()` call.
- **Current impact**: Low — only affects YAML policy loading, which is an optional feature.
- **Suggestion**: Remove the explicit `exists()` check. Instead, attempt the read directly and handle `NotFound` error gracefully.

### [Warning] Path constructed via `format!` instead of `PathBuf::join`
- **Location**: `src/daemon/platforms/launchd.rs:20-23`
- **Risk**: Path separators may be incorrect on non-Unix systems (though this is macOS-only code). Also, if `home` contains special characters, direct string interpolation is less safe than `PathBuf` operations.
- **Current impact**: Low — macOS only, `$HOME` is typically safe.
- **Suggestion**: Use `PathBuf::from(&home).join("Library").join("LaunchAgents").join(format!("{}.plist", LAUNCHD_LABEL))`.

### [Warning] `line` string reused without capacity trimming in IPC server
- **Location**: `src/daemon/ipc/server.rs:56,59-60`
- **Risk**: `line.clear()` resets length to 0 but preserves capacity. If a malicious client sends an extremely long line (> 1MB), subsequent iterations reuse the large buffer, causing unbounded memory growth.
- **Current impact**: Medium — IPC socket is local-only, but a compromised local process could exploit this.
- **Suggestion**: After `line.clear()`, add `line.shrink_to(1024);` to cap capacity, or use a bounded read mechanism.

### [Warning] `PendingAction::id()` uses non-cryptographic hash for uniqueness
- **Location**: `src/daemon/worldmodel/state.rs:91-103`
- **Risk**: SHA-256 truncated to 16 hex chars provides only 64 bits of entropy. With sufficient actions, collision probability becomes non-negligible. Also, `serde_json::to_string` on `ActionType` may not be stable across versions.
- **Current impact**: Low — only affects pending action deduplication, which is not security-critical.
- **Suggestion**: Use a proper UUID library or at least increase truncation length to 32 chars (128 bits).

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 5 |
| Suggested Test | 0 |

---

## Cross-Module Findings

None — daemon module is self-contained for this audit.
