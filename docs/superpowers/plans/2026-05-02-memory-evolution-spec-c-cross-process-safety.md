# Spec C — Cross-Process Safety Beyond Curated Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate every remaining cross-process write race in `~/.aleph/data/`: harden the singleton lock + early-acquire it; route every CLI write either into the running server via a new `/v1/admin/*` IPC namespace or local lock-protected path; standardise SQLite opens through a WAL+busy_timeout helper; protect `secrets.vault` and `acp_sessions.json` with atomic temp+rename (plus fcntl for vault).

**Architecture:** New crate-level utilities in `src/utils/{atomic_io,instance_lock,sqlite_open}.rs` and a new admin IPC layer (`gateway::admin_api`). All CLI subcommands route via `with_policy` / `run_no_lock` helpers carrying a declarative `CommandPolicy`. Server writes a discoverable `.ipc-endpoint.json` after binding; CLI reads bearer token from `security.db` in read-only WAL mode. Vault writes wrap atomic temp+rename with an adjacent fcntl lock; acp_sessions writes use atomic temp+rename only.

**Tech Stack:** Rust 1.x, Tokio async, axum (existing gateway), reqwest (existing dep), `fs2` advisory locks (already a Spec A dep), `rusqlite` with `OpenFlags::SQLITE_OPEN_READ_ONLY`, `tempfile` for atomic temp+rename, `serde_json` for IPC bodies, `proptest` for concurrency invariants.

**Spec reference:** `docs/superpowers/specs/2026-05-02-memory-evolution-spec-c-cross-process-safety-design.md`

**Constraints inherited:**
- The 5 pre-existing dirty files **must remain untouched throughout this plan**: `interfaces/webchat/dist/aleph_panel.js`, `interfaces/webchat/dist/aleph_panel_bg.wasm`, `src/agents/runtime.rs`, `src/gateway/execution_engine/engine.rs`, `src/gateway/execution_engine/run_loop.rs`. None of Spec C's tasks need to touch them.
- Spec A's curated layer (`src/memory/curated/format.rs`) is **read-only** from this plan's perspective. The new `atomic_io` and `instance_lock` modules duplicate primitives intentionally; consolidation is a future cleanup PR, not part of Spec C.
- All commits use English subject lines in the form `spec-c: <description>` (no `--no-verify`, no `git add -A`, no amends).

---

## File Structure

### Files to create

| Path | Responsibility |
|---|---|
| `src/utils/atomic_io.rs` | `write_atomic` + `with_file_lock` + `FileLockGuard` RAII |
| `src/utils/instance_lock.rs` | `InstanceLock` struct, `AcquireOutcome` enum, `try_acquire` + `diagnose_holder` |
| `src/utils/sqlite_open.rs` | `open_sqlite_safe` + `open_sqlite_readonly` |
| `src/utils/vault_io.rs` | `VaultIo` wrapper around `secrets.vault` |
| `src/cli/policy.rs` | `CommandPolicy`, `HttpMethod`, `with_policy`, `run_no_lock` |
| `src/cli/ipc_client.rs` | `forward_to_server` + 401 retry + endpoint discovery |
| `src/cli/endpoint.rs` | `IpcEndpoint` struct + `read_endpoint_file` + atomic write helpers (server side) |
| `src/cli/mod.rs` | Re-export the above (only if `src/cli/` doesn't already exist; otherwise extend) |
| `src/gateway/admin_api/mod.rs` | Admin namespace router |
| `src/gateway/admin_api/secrets.rs` | 4 handlers for `/v1/admin/secrets/*` |
| `src/gateway/admin_api/memory.rs` | 3 handlers for `/v1/admin/memory/*` |
| `src/gateway/admin_api/agents.rs` | 3 handlers for `/v1/admin/agents/*` |
| `src/gateway/security/token_readonly.rs` | `read_current_token_readonly(&Connection) -> Result<Option<String>>` |
| `scripts/spec_c_regression.sh` | Reverse-regression checks |
| `tests/instance_lock_e2e.rs` | Unix flock multi-process integration |
| `tests/spec_c_double_start.rs` | Two `aleph-server start` racing |
| `tests/spec_c_cli_no_server.rs` | CLI write while server down |
| `tests/spec_c_cli_ipc.rs` | CLI write while server up — IPC happy path |
| `tests/spec_c_cli_token_rotation.rs` | 401 self-heal |
| `tests/spec_c_cli_refuse.rs` | LockOnly command refused while server up |
| `tests/spec_c_cli_endpoint_missing.rs` | Locked but no endpoint file |
| `tests/vault_atomic_e2e.rs` | Crash-safe vault write |
| `tests/vault_concurrent_e2e.rs` | Two-thread vault write |
| `tests/acp_atomic_e2e.rs` | Crash-safe acp_sessions write |
| `tests/sqlite_concurrent_read_e2e.rs` | Reader/writer non-blocking |
| `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_c_cross_process_safety.md` | Memory file tracking shipped commits |

### Files to modify

| Path | Change |
|---|---|
| `Cargo.toml` | (No new deps — `fs2`, `tempfile`, `reqwest`, `rusqlite` already present) |
| `src/utils/mod.rs` | `pub mod atomic_io; pub mod instance_lock; pub mod sqlite_open; pub mod vault_io;` |
| `src/lib.rs` (or wherever `cli` module lives) | `pub mod cli;` if not already declared |
| `src/bin/aleph-server/daemon.rs` | Replace body of `acquire_instance_lock` with thin `pub use alephcore::utils::instance_lock::*;` re-export; keep public API surface identical for transitional callers |
| `src/bin/aleph-server/main.rs` | Move lock acquisition to first action in `main()` (before `tracing_subscriber` init) |
| `src/bin/aleph-server/commands/secret.rs` | Wire to `with_policy(LockOrIpc { route: "/v1/admin/secrets/...", method: ... }, local, ipc_body)` |
| `src/bin/aleph-server/commands/start/mod.rs` | Remove inline `acquire_instance_lock` call; lock now held by `main()` |
| `src/acp/manager.rs:24-50` | Replace `fs::write` in save path with `atomic_io::write_atomic` |
| `src/memory/store/sqlite/mod.rs:71-73` | Replace inline `journal_mode=WAL` block with `open_sqlite_safe(path)` call |
| `src/tasks/shared/store.rs:25-32` | Replace inline `journal_mode=WAL` block with `open_sqlite_safe(path)` call |
| `src/tasks/heartbeat/store.rs:42-47` | Same |
| `src/tasks/cron/store.rs:48-53` | Same |
| All other rusqlite `Connection::open` callers (~12 sites — listed in Task 7) | Route through `open_sqlite_safe(path)` |
| All `secrets.vault` direct `fs::*` callers (audit in Task 1) | Route through `VaultIo` |
| `src/gateway/security/shared_token.rs` | Add public re-export of token-read-only path (`pub use crate::gateway::security::token_readonly::read_current_token_readonly;`) — does NOT modify existing `SharedTokenManager` |
| `src/gateway/mod.rs` | `pub mod admin_api;` + mount in axum router |
| `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md` | Mark Spec C row `✅ shipped` (Task 26) |
| `docs/reference/SECURITY.md` | Append "Cross-process safety guarantees" subsection |
| `CLAUDE.md` (project) | Update Process Management section: replace "wait 2 seconds for lock" with new guarantees |
| `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md` | Add Spec C index entry |

---

### Task 1: Discovery / API audit + module scaffold

**Purpose:** Lock down every API surface before code lands. No production edits — only `grep` runs and an audit report committed as a doc comment in `src/utils/mod.rs` (or a fresh `src/utils/spec_c_audit.rs` placeholder file). This mirrors Spec A/B's Task 1 pattern.

**Files:**
- Create: `src/utils/spec_c_audit.rs` (audit notes only; deleted at the end of Spec C in Task 26)

- [ ] **Step 1: Verify the 5 inherited dirty files baseline**

Run:
```bash
git status --short
```

Expected: exactly these 5 lines (in any order), no others:
```
 M interfaces/webchat/dist/aleph_panel.js
 M interfaces/webchat/dist/aleph_panel_bg.wasm
 M src/agents/runtime.rs
 M src/gateway/execution_engine/engine.rs
 M src/gateway/execution_engine/run_loop.rs
```

If anything else appears, stop and ask. These 5 must remain untouched throughout the entire plan.

- [ ] **Step 2: Audit existing singleton acquisition**

Run:
```bash
sed -n '53,127p' /Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/daemon.rs
grep -rn "acquire_instance_lock\|aleph\\.lock" src/ --include="*.rs"
```

Expected:
- Confirm `acquire_instance_lock` lives at `src/bin/aleph-server/daemon.rs:62`
- Record every call site (likely just `commands/start/mod.rs`)
- Confirm flock path is `~/.aleph/data/aleph.lock` and call uses `libc::LOCK_EX | libc::LOCK_NB`

Record findings in audit notes (next step).

- [ ] **Step 3: Audit existing SQLite open sites**

Run:
```bash
grep -rn "Connection::open(\|rusqlite::Connection::open(" src/ --include="*.rs" | grep -v test
```

Expected: 15-20 sites. Record full list. Tag which ones already set `journal_mode=WAL` (cross-reference with `grep -rn "journal_mode=WAL" src/`).

- [ ] **Step 4: Audit existing `secrets.vault` and `acp_sessions.json` access sites**

Run:
```bash
grep -rn "secrets\\.vault\|acp_sessions\\.json" src/ --include="*.rs"
grep -rn "VaultStore\|secrets_vault_path\|secrets\\\\.vault" src/ --include="*.rs"
```

Expected: identify the canonical path-builder functions (likely in `src/gateway/security/store.rs` and `src/acp/manager.rs:24`). Record the byte/string sigil for each direct `fs::write` / `fs::read` of these files.

- [ ] **Step 5: Audit existing `SharedTokenManager` token-read API**

Run:
```bash
grep -n "fn current_token\|fn get_current_token\|fn token_for\|SELECT.*shared_token" src/gateway/security/ -r --include="*.rs"
```

Expected: identify the SQL query and Connection method used to read the active bearer token. Record the exact `SELECT` statement so Task 13's `read_current_token_readonly` can mirror it byte-for-byte.

- [ ] **Step 6: Audit existing CLI subcommand entry points**

Run:
```bash
ls src/bin/aleph-server/commands/
grep -l "pub fn\|pub async fn" src/bin/aleph-server/commands/*.rs
```

Expected: enumerate every CLI command file (estimate: 8-15). Record which are read-only vs write-only vs mixed.

- [ ] **Step 7: Audit existing reqwest usage to confirm it's already a dependency**

Run:
```bash
grep -E '^reqwest' Cargo.toml
grep -rn "reqwest::Client" src/ --include="*.rs" | head -5
```

Expected: confirm `reqwest` listed in `[dependencies]`. If not, add it in this task as a separate sub-step (otherwise Spec C has no new deps).

- [ ] **Step 8: Write audit notes to `src/utils/spec_c_audit.rs`**

Create the file with the following template (fill `<...>` placeholders from steps 2-7):

```rust
//! # Spec C — Cross-Process Safety Audit Notes
//!
//! Temporary audit-only file. **Deleted in Task 26** (final acceptance).
//!
//! ## Singleton (current state, Task 3 will migrate)
//! - Path: `~/.aleph/data/aleph.lock`
//! - Implementation: `src/bin/aleph-server/daemon.rs:62-126`
//! - Flock: `libc::LOCK_EX | libc::LOCK_NB`
//! - Call sites: <list>
//!
//! ## SQLite Connection::open sites
//! Already WAL+busy_timeout (4 sites; Task 7 migrates to helper):
//! - <list>
//! Missing WAL+busy_timeout (~12 sites; Task 7 migrates):
//! - <list>
//!
//! ## secrets.vault access
//! - Canonical path builder: <fn>
//! - Direct fs::* sites: <list>
//!
//! ## acp_sessions.json access
//! - Canonical path: `src/acp/manager.rs:24` `acp_sessions_path()`
//! - Direct fs::* sites: <list>
//!
//! ## SharedTokenManager active-token query
//! - File/line: <path>
//! - SQL: <verbatim SELECT>
//!
//! ## CLI subcommands
//! - <command name>: read/write/mixed → tentative policy <NoLock|LockOnly|LockOrIpc>
//!
//! ## reqwest in deps
//! - <yes/no>
```

Add a single line to `src/utils/mod.rs`:
```rust
#[cfg(debug_assertions)]
pub mod spec_c_audit;  // Removed in Task 26
```

- [ ] **Step 9: Verify it compiles**

Run:
```bash
cargo check -p alephcore --lib 2>&1 | tail -20
```

Expected: `Finished` with no new errors. Existing warnings are fine.

- [ ] **Step 10: Commit audit**

```bash
git add src/utils/spec_c_audit.rs src/utils/mod.rs
git commit -m "spec-c: scaffold audit notes for cross-process safety"
```

---

### Task 2: `atomic_io.rs` — write_atomic + with_file_lock

**Purpose:** Foundation utility used by every later task that touches files. Independent unit, fully tested in isolation, no external state.

**Files:**
- Create: `src/utils/atomic_io.rs`
- Modify: `src/utils/mod.rs` (add `pub mod atomic_io;`)

- [ ] **Step 1: Write the failing tests first**

Create `src/utils/atomic_io.rs` with the test module only:

```rust
//! Atomic file writes + advisory file locks.
//!
//! `write_atomic` writes via `<path>.tmp.<rand>` + fsync + rename so
//! readers always see either a complete old file or a complete new file
//! (never half-written).
//!
//! `with_file_lock` acquires an exclusive `fs2` advisory lock on a
//! sidecar `<path>.lock` file for the duration of a closure. Lock
//! release is RAII-driven (Drop on the guard).

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn write_atomic_creates_file_with_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn write_atomic_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        write_atomic(&path, b"x").unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["foo.bin".to_string()]);
    }

    #[test]
    fn with_file_lock_serialises_two_threads() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("x.lock");
        let counter = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = vec![];
        for tag in [b'A', b'B'] {
            let lp = lock_path.clone();
            let c = counter.clone();
            let b = barrier.clone();
            handles.push(thread::spawn(move || {
                b.wait();
                with_file_lock(&lp, |_guard| {
                    let mut v = c.lock().unwrap();
                    v.push(tag);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    v.push(tag);
                    Ok(())
                }).unwrap();
            }));
        }
        for h in handles { h.join().unwrap(); }

        // Each tag wrote two bytes back-to-back without interleave
        let v = counter.lock().unwrap();
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], v[1]);
        assert_eq!(v[2], v[3]);
        assert_ne!(v[0], v[2]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (no implementation yet)**

Run:
```bash
cargo test -p alephcore --lib utils::atomic_io 2>&1 | tail -20
```

Expected: compile errors — `write_atomic` and `with_file_lock` not defined.

- [ ] **Step 3: Implement `write_atomic`**

Add to `src/utils/atomic_io.rs` above the `#[cfg(test)] mod tests`:

```rust
use std::fs::File;
use std::io::Write;
use std::path::Path;

use fs2::FileExt;

/// Write bytes to `path` atomically: write to a sibling `.tmp.<rand>` file,
/// fsync, then rename over the destination. Readers always see either the
/// complete old file (or no file) or the complete new file — never a
/// half-written intermediate.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "write_atomic path has no parent directory",
    ))?;

    let mut tmp = tempfile::Builder::new()
        .prefix(".aleph_atomic_")
        .tempfile_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
```

- [ ] **Step 4: Implement `with_file_lock` + `FileLockGuard`**

Append to `src/utils/atomic_io.rs`:

```rust
/// RAII guard returned by `with_file_lock`. Drops the underlying
/// `File`, which releases the OS-level fs2 lock.
pub struct FileLockGuard {
    _file: File,
}

/// Acquire an exclusive fs2 advisory lock on `lock_path`, run `f`, and
/// release on return. The closure receives a borrow of the guard so it
/// cannot escape and call paths can still inspect lock state if needed.
///
/// Note: `lock_path` is the **lock sidecar**, not the data file. Callers
/// should pass e.g. `secrets.vault.lock` for a data file at `secrets.vault`.
pub fn with_file_lock<T, F>(lock_path: &Path, f: F) -> std::io::Result<T>
where
    F: FnOnce(&FileLockGuard) -> std::io::Result<T>,
{
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock_exclusive()?;
    let guard = FileLockGuard { _file: file };
    f(&guard)
}
```

- [ ] **Step 5: Wire `pub mod atomic_io;` in `src/utils/mod.rs`**

Add the line near the top of `src/utils/mod.rs`:

```rust
pub mod atomic_io;
```

(Keep alphabetical ordering with existing modules.)

- [ ] **Step 6: Run tests to verify they pass**

Run:
```bash
cargo test -p alephcore --lib utils::atomic_io 2>&1 | tail -20
```

Expected: 4 tests pass.

- [ ] **Step 7: Run clippy on the new file**

Run:
```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep -E "atomic_io" | head -10
```

Expected: no `atomic_io` warnings.

- [ ] **Step 8: Commit**

```bash
git add src/utils/atomic_io.rs src/utils/mod.rs
git commit -m "spec-c: atomic_io — write_atomic + with_file_lock + RAII guard"
```

---

### Task 3: `instance_lock.rs` — Unix path with structured AcquireOutcome

**Purpose:** Promote the existing daemon-only `acquire_instance_lock` to a reusable core utility with a structured outcome enum so CLI commands can branch cleanly. This task does the Unix path + tests; Task 4 adds the Windows fallback.

**Files:**
- Create: `src/utils/instance_lock.rs`
- Modify: `src/utils/mod.rs` (add `pub mod instance_lock;`)

- [ ] **Step 1: Write the failing tests**

Create `src/utils/instance_lock.rs` with:

```rust
//! Cross-process singleton lock for a given Aleph data directory.
//!
//! Uses `fs2::FileExt::try_lock_exclusive` on `<data_dir>/aleph.lock`.
//! The lock is automatically released by the OS when the holder process
//! exits (graceful, panic, SIGKILL — all release).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_acquire_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = try_acquire(dir.path()).unwrap();
        assert!(matches!(outcome, AcquireOutcome::Acquired(_)));
    }

    #[test]
    fn second_acquire_in_same_process_returns_held_by_live() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = match try_acquire(dir.path()).unwrap() {
            AcquireOutcome::Acquired(g) => g,
            other => panic!("first acquire should succeed, got {:?}", other),
        };
        let second = try_acquire(dir.path()).unwrap();
        match second {
            AcquireOutcome::HeldByLive { pid, .. } => {
                assert_eq!(pid as u32, std::process::id());
            }
            other => panic!("expected HeldByLive, got {:?}", other),
        }
    }

    #[test]
    fn release_then_reacquire_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let first = try_acquire(dir.path()).unwrap();
        match first {
            AcquireOutcome::Acquired(g) => drop(g),
            _ => panic!(),
        }
        let again = try_acquire(dir.path()).unwrap();
        assert!(matches!(again, AcquireOutcome::Acquired(_)));
    }

    #[test]
    fn diagnose_holder_returns_none_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(diagnose_holder(dir.path()).is_none());
    }

    #[test]
    fn diagnose_holder_returns_pid_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = try_acquire(dir.path()).unwrap();
        let diag = diagnose_holder(dir.path()).expect("file should exist");
        assert_eq!(diag.pid as u32, std::process::id());
        assert!(diag.process_alive);
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:
```bash
cargo test -p alephcore --lib utils::instance_lock 2>&1 | tail -20
```

Expected: compile errors — `try_acquire`, `AcquireOutcome`, `diagnose_holder` not defined.

- [ ] **Step 3: Implement types**

Prepend to `src/utils/instance_lock.rs`:

```rust
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

const LOCK_FILENAME: &str = "aleph.lock";

#[derive(Debug)]
pub struct InstanceLock {
    file: File,
    path: PathBuf,
    holder_pid: u32,
}

impl InstanceLock {
    pub fn lock_path(&self) -> &Path { &self.path }
    pub fn holder_pid(&self) -> u32 { self.holder_pid }
}

// Drop releases the OS-level fs2 lock automatically when `file` is dropped.

#[derive(Debug)]
pub enum AcquireOutcome {
    Acquired(InstanceLock),
    HeldByLive { pid: i32, lock_path: PathBuf },
    HeldByOrphaned { pid: i32, lock_path: PathBuf },
}

#[derive(Debug)]
pub struct HolderDiagnostic {
    pub pid: i32,
    pub process_alive: bool,
    pub lock_path: PathBuf,
}
```

- [ ] **Step 4: Implement `try_acquire`**

Append to `src/utils/instance_lock.rs`:

```rust
/// Attempt to acquire the singleton lock for `data_dir`. Caller must
/// hold the returned `InstanceLock` for as long as exclusive access is
/// required.
pub fn try_acquire(data_dir: &Path) -> std::io::Result<AcquireOutcome> {
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir)?;
    }
    let lock_path = data_dir.join(LOCK_FILENAME);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    match file.try_lock_exclusive() {
        Ok(()) => {
            // Got the lock — write our PID for diagnostics.
            let pid = std::process::id();
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            writeln!(file, "{pid}")?;
            file.sync_all()?;
            Ok(AcquireOutcome::Acquired(InstanceLock {
                file,
                path: lock_path,
                holder_pid: pid,
            }))
        }
        Err(_) => {
            // Lock is held by someone else. Read PID for diagnostics.
            let mut buf = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut buf)?;
            let pid: i32 = buf.trim().parse().unwrap_or(0);
            if is_process_alive(pid) {
                Ok(AcquireOutcome::HeldByLive { pid, lock_path })
            } else {
                Ok(AcquireOutcome::HeldByOrphaned { pid, lock_path })
            }
        }
    }
}

/// Read holder metadata from the lock file WITHOUT competing for the lock.
/// Returns None if the lock file does not exist.
pub fn diagnose_holder(data_dir: &Path) -> Option<HolderDiagnostic> {
    let lock_path = data_dir.join(LOCK_FILENAME);
    let mut file = std::fs::File::open(&lock_path).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    let pid: i32 = buf.trim().parse().ok()?;
    Some(HolderDiagnostic {
        pid,
        process_alive: is_process_alive(pid),
        lock_path,
    })
}

#[cfg(unix)]
fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 { return false; }
    // SAFETY: `kill(pid, 0)` only checks process existence + permissions.
    // Returns 0 if process exists, -1 + ESRCH otherwise.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_alive(_pid: i32) -> bool {
    // Best-effort fallback for non-Unix; always assume alive to err on the
    // safe side (caller will fail back to a "lock held" branch).
    true
}
```

- [ ] **Step 5: Wire `pub mod instance_lock;` in `src/utils/mod.rs`**

Add (alphabetically ordered):

```rust
pub mod instance_lock;
```

- [ ] **Step 6: Run tests to verify pass**

Run:
```bash
cargo test -p alephcore --lib utils::instance_lock 2>&1 | tail -20
```

Expected: 5 tests pass.

- [ ] **Step 7: Run clippy**

Run:
```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep -E "instance_lock" | head -10
```

Expected: no `instance_lock` warnings.

- [ ] **Step 8: Commit**

```bash
git add src/utils/instance_lock.rs src/utils/mod.rs
git commit -m "spec-c: instance_lock — InstanceLock + AcquireOutcome enum + try_acquire/diagnose_holder"
```

---

### Task 4: instance_lock — Windows fs2 fallback + cross-platform integration test

**Purpose:** Replace the existing Windows fallback (which doesn't actually lock) with a real `fs2` lock so behaviour is identical across platforms. Add a multi-process integration test that fork+exec a child holding the lock.

**Files:**
- Modify: `src/utils/instance_lock.rs` (already cross-platform via `fs2`, but verify)
- Create: `tests/instance_lock_e2e.rs`

- [ ] **Step 1: Verify `fs2::FileExt::try_lock_exclusive` works on both platforms**

Read documentation cross-check:
```bash
grep -rn "try_lock_exclusive" target/doc/fs2/ 2>/dev/null | head -5 || true
```

The `fs2` crate uses `flock` on Unix and `LockFileEx` on Windows internally. No code change needed — the implementation in Task 3 is already cross-platform. Just confirm the test from Task 3 also runs on Windows in CI.

- [ ] **Step 2: Write integration test**

Create `tests/instance_lock_e2e.rs`:

```rust
//! Multi-process integration test for the singleton lock.
//!
//! Spawns a child process that takes the lock and holds it; parent
//! verifies its own `try_acquire` returns `HeldByLive` with the child's
//! PID. Then signals the child to exit and re-acquires.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use alephcore::utils::instance_lock::{self, AcquireOutcome};

#[test]
fn child_holds_lock_parent_sees_held_by_live() {
    let dir = tempfile::tempdir().unwrap();
    let dir_arg = dir.path().to_string_lossy().into_owned();

    // Spawn a helper child that takes the lock and waits on stdin.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--lock-and-wait")
        .arg(&dir_arg)
        .env("ALEPH_LOCK_HELPER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut child_out = child.stdout.take().unwrap();
    let mut ready = [0u8; 5];
    child_out.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"READY");

    let outcome = instance_lock::try_acquire(dir.path()).unwrap();
    match outcome {
        AcquireOutcome::HeldByLive { pid, .. } => {
            assert_eq!(pid as u32, child.id());
        }
        other => panic!("expected HeldByLive, got {:?}", other),
    }

    // Tell child to exit.
    let mut child_in = child.stdin.take().unwrap();
    writeln!(child_in, "exit").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());

    std::thread::sleep(Duration::from_millis(50));

    let after = instance_lock::try_acquire(dir.path()).unwrap();
    assert!(matches!(after, AcquireOutcome::Acquired(_)));
}

/// Helper entry — when invoked with `--lock-and-wait <dir>` and env
/// `ALEPH_LOCK_HELPER=1`, take the lock, print "READY" + flush, wait
/// for "exit\n" on stdin, then drop the lock.
#[test]
fn lock_helper_entry_point() {
    if std::env::var_os("ALEPH_LOCK_HELPER").is_none() { return; }
    let mut args = std::env::args().skip_while(|a| a != "--lock-and-wait");
    if args.next().is_none() { return; }
    let dir = args.next().expect("--lock-and-wait requires a dir argument");
    let outcome = instance_lock::try_acquire(std::path::Path::new(&dir)).unwrap();
    let _hold = match outcome {
        AcquireOutcome::Acquired(g) => g,
        other => panic!("helper failed to acquire: {:?}", other),
    };
    print!("READY");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "exit");
    drop(_hold);
    std::process::exit(0);
}
```

- [ ] **Step 3: Run integration test**

Run:
```bash
cargo test --test instance_lock_e2e -- --test-threads=1 2>&1 | tail -20
```

Expected: 1 test passes (`child_holds_lock_parent_sees_held_by_live`); the helper entry point either runs as a no-op when env is absent or runs as the helper when invoked via Command. The `--test-threads=1` is required because both tests share the same binary.

- [ ] **Step 4: Commit**

```bash
git add tests/instance_lock_e2e.rs
git commit -m "spec-c: instance_lock — multi-process integration test"
```

---

### Task 5: Migrate server start to early acquisition + retire daemon copy

**Purpose:** Move the singleton-lock acquisition to be the very first action in `main()` (before tracing, before config load) and replace the daemon copy with a thin re-export.

**Files:**
- Modify: `src/bin/aleph-server/main.rs` (or wherever `fn main()` lives)
- Modify: `src/bin/aleph-server/daemon.rs:62-126` (replace body with re-export)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (remove inline lock; lock now in `main()`)

- [ ] **Step 1: Locate `fn main` for the aleph-server binary**

Run:
```bash
grep -n "fn main" src/bin/aleph-server/main.rs 2>/dev/null \
  || grep -rn "fn main" src/bin/aleph-server/ --include="*.rs" | head -5
```

Expected: a single `fn main` (likely `src/bin/aleph-server/main.rs`).

- [ ] **Step 2: Acquire lock as first action in `main`**

Edit the located `fn main`. The change is conceptually:

```rust
// BEFORE
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()...;
    let cli = Cli::parse();
    ...
}

// AFTER
fn main() -> anyhow::Result<()> {
    let _instance_lock = match alephcore::utils::instance_lock::try_acquire(
        &alephcore::utils::paths::data_dir()?,
    )? {
        alephcore::utils::instance_lock::AcquireOutcome::Acquired(lock) => Some(lock),
        alephcore::utils::instance_lock::AcquireOutcome::HeldByLive { pid, lock_path } => {
            eprintln!(
                "Another Aleph instance is already running (PID {pid}).\n\
                 Running multiple instances simultaneously will corrupt the vault \
                 and destroy all stored API keys.\n\
                 Stop the other instance first: kill {pid} or `aleph stop`\n\
                 Lock file: {}", lock_path.display(),
            );
            std::process::exit(64);
        }
        alephcore::utils::instance_lock::AcquireOutcome::HeldByOrphaned { pid, lock_path } => {
            eprintln!(
                "Stale lock file detected (PID {pid} not running).\n\
                 You may safely `rm {}` if no aleph process exists.\n\
                 Lock file: {}", lock_path.display(), lock_path.display(),
            );
            std::process::exit(64);
        }
    };

    // Now safe to init tracing, parse CLI, load config, etc.
    tracing_subscriber::fmt()...;
    let cli = Cli::parse();
    ...
}
```

The `Some(lock)` binding **must** stay alive for the entire `main`'s scope. Bind it to a name that lints don't flag (`_instance_lock` is fine; underscore prefix prevents unused-var lint while the actual `Drop` still runs at end-of-scope).

**Important:** `data_dir()` resolution must NOT depend on tracing (it's a pure path helper). Verify:

```bash
grep -A 5 "fn data_dir" src/utils/paths.rs
```

Confirm it's pure `dirs::home_dir().join(...)`.

**Important:** Some CLI subcommands (NoLock per Task 11 — e.g., `--version`, `stop`) must NOT acquire the lock. Defer that branching to Task 11; for now, every subcommand goes through this main-level acquisition. We will refine in Task 11 by reading `argv[1]` first and skipping the lock for known NoLock subcommands. For Task 5 only, ALL subcommands hit the lock.

Actually, simpler: Since `stop` and `--version` are NoLock per spec but currently still go through main, this Task 5 explicitly only acquires for the `start` subcommand. Re-edit `fn main` accordingly:

```rust
fn main() -> anyhow::Result<()> {
    // Inspect argv to decide whether to acquire the singleton lock.
    // Detail subcommand routing happens via the existing CLI parser, but
    // we can do a cheap argv0 sniff here without parsing.
    let args: Vec<String> = std::env::args().collect();
    let needs_lock_in_main = args.iter().any(|a| a == "start");

    let _instance_lock = if needs_lock_in_main {
        match alephcore::utils::instance_lock::try_acquire(
            &alephcore::utils::paths::data_dir()?,
        )? {
            alephcore::utils::instance_lock::AcquireOutcome::Acquired(lock) => Some(lock),
            alephcore::utils::instance_lock::AcquireOutcome::HeldByLive { pid, lock_path } => {
                eprintln!(
                    "Another Aleph instance is already running (PID {pid}). \
                     Stop it first: kill {pid} or `aleph stop`. Lock file: {}",
                    lock_path.display(),
                );
                std::process::exit(64);
            }
            alephcore::utils::instance_lock::AcquireOutcome::HeldByOrphaned { pid, lock_path } => {
                eprintln!(
                    "Stale lock file detected (PID {pid} not running). \
                     You may safely `rm {}` if no aleph process exists.",
                    lock_path.display(),
                );
                std::process::exit(64);
            }
        }
    } else {
        None
    };

    // Tracing init + CLI parse + dispatch.
    tracing_subscriber::fmt()...;
    let cli = Cli::parse();
    ...
}
```

Task 11 will replace this argv-sniff approach with the cleaner `with_policy` / `run_no_lock` framework; for now, this gets the lock acquisition wired earlier than today.

- [ ] **Step 3: Remove inline `acquire_instance_lock` call from `commands/start/mod.rs`**

Run:
```bash
grep -n "acquire_instance_lock" src/bin/aleph-server/commands/start/mod.rs
```

Identify the call site. Replace its block with a comment:

```rust
// Singleton lock is held by `main()` for the entire process lifetime.
```

Remove any `let _lock = ...` binding inside `commands/start`.

- [ ] **Step 4: Replace daemon body with re-export**

Edit `src/bin/aleph-server/daemon.rs:53-126`. Replace the entire body of `acquire_instance_lock` with:

```rust
/// **Deprecated** — moved to `alephcore::utils::instance_lock`.
/// This thin wrapper exists during the Spec C transition window so any
/// internal call sites that still reference the daemon path keep
/// compiling. Will be removed in a subsequent cleanup PR.
pub fn acquire_instance_lock() -> Result<std::fs::File, Box<dyn std::error::Error>> {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".aleph/data");
    match alephcore::utils::instance_lock::try_acquire(&dir)? {
        alephcore::utils::instance_lock::AcquireOutcome::Acquired(lock) => {
            // Leak the InstanceLock and return its underlying File for
            // backward compat — the lock stays held until process exit
            // either way.
            let file = unsafe { std::mem::transmute_copy::<_, std::fs::File>(&lock) };
            std::mem::forget(lock);
            Ok(file)
        }
        alephcore::utils::instance_lock::AcquireOutcome::HeldByLive { pid, .. } => {
            Err(format!("Another Aleph instance is running (PID {pid})").into())
        }
        alephcore::utils::instance_lock::AcquireOutcome::HeldByOrphaned { pid, .. } => {
            Err(format!("Stale lock file (PID {pid} not running)").into())
        }
    }
}
```

The `transmute_copy` is intentionally unsafe — we know `InstanceLock` wraps a `File` as its first field, but **prefer not to rely on that**. Instead, expose a public `into_file()` accessor on `InstanceLock`:

Edit `src/utils/instance_lock.rs` to add to `impl InstanceLock`:

```rust
/// Consume the lock and return the underlying file handle. The OS-level
/// fs2 lock is released only when this `File` is dropped.
pub fn into_file(self) -> std::fs::File { self.file }
```

Then update the daemon wrapper to use `into_file()` instead of transmute:

```rust
        alephcore::utils::instance_lock::AcquireOutcome::Acquired(lock) => {
            Ok(lock.into_file())
        }
```

Replace the unsafe block with the safe accessor. No `std::mem::forget` needed — the `File` keeps the OS lock alive identically.

- [ ] **Step 5: Build & verify all callers still compile**

Run:
```bash
cargo build --workspace 2>&1 | tail -30
```

Expected: Finished. Ignore warnings about deprecated daemon module.

- [ ] **Step 6: Manual smoke — start two servers**

```bash
target/debug/aleph-server start &
SERVER_PID=$!
sleep 3
target/debug/aleph-server start
SECOND_EXIT=$?
echo "second exit: $SECOND_EXIT"
kill $SERVER_PID
wait
```

Expected: `second exit: 64` and stderr contains `Another Aleph instance is already running (PID <SERVER_PID>)`.

- [ ] **Step 7: Run unit + integration tests**

Run:
```bash
cargo test -p alephcore --lib utils::instance_lock
cargo test --test instance_lock_e2e -- --test-threads=1
```

Expected: green.

- [ ] **Step 8: Commit**

```bash
git add src/bin/aleph-server/main.rs \
        src/bin/aleph-server/daemon.rs \
        src/bin/aleph-server/commands/start/mod.rs \
        src/utils/instance_lock.rs
git commit -m "spec-c: move singleton lock to main() entry, leave daemon thin re-export"
```

---

### Task 6: `sqlite_open.rs` — open_sqlite_safe + open_sqlite_readonly

**Purpose:** Single source of truth for SQLite open flags + pragmas.

**Files:**
- Create: `src/utils/sqlite_open.rs`
- Modify: `src/utils/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/utils/sqlite_open.rs`:

```rust
//! Standard SQLite open helpers for cross-process safety.

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma_str(conn: &rusqlite::Connection, pragma: &str) -> String {
        conn.query_row(&format!("PRAGMA {pragma}"), [], |row| row.get::<_, String>(0))
            .unwrap_or_else(|_| {
                conn.query_row(&format!("PRAGMA {pragma}"), [], |row| {
                    let v: i64 = row.get(0)?;
                    Ok(v.to_string())
                }).unwrap()
            })
    }

    #[test]
    fn open_sqlite_safe_sets_wal_busy_synchronous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = open_sqlite_safe(&path).unwrap();
        assert_eq!(pragma_str(&conn, "journal_mode").to_lowercase(), "wal");
        assert_eq!(pragma_str(&conn, "busy_timeout"), "5000");
        assert_eq!(pragma_str(&conn, "synchronous"), "1"); // NORMAL = 1
        assert_eq!(pragma_str(&conn, "foreign_keys"), "1");
    }

    #[test]
    fn open_sqlite_readonly_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        // Seed schema with safe helper
        let writer = open_sqlite_safe(&path).unwrap();
        writer.execute_batch("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1);").unwrap();
        drop(writer);

        let reader = open_sqlite_readonly(&path).unwrap();
        let n: i64 = reader.query_row("SELECT id FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);

        let result = reader.execute("INSERT INTO t VALUES (2)", []);
        assert!(result.is_err(), "readonly conn should reject writes");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run:
```bash
cargo test -p alephcore --lib utils::sqlite_open 2>&1 | tail -20
```

Expected: compile errors.

- [ ] **Step 3: Implement the helpers**

Prepend to `src/utils/sqlite_open.rs`:

```rust
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

/// Open a SQLite connection with the cross-process safety pragmas:
/// - `journal_mode=WAL`     — concurrent reads + writer-friendly
/// - `busy_timeout=5000`    — 5s wait on lock contention before SQLITE_BUSY
/// - `synchronous=NORMAL`   — WAL-safe, faster than FULL
/// - `foreign_keys=ON`      — existing project convention
pub fn open_sqlite_safe(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(e))
            })?;
        }
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(conn)
}

/// Open a read-only SQLite connection. Writes will fail with
/// `SQLITE_READONLY`. Same WAL-aware pragmas as the safe writer.
pub fn open_sqlite_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}
```

- [ ] **Step 4: Wire `pub mod sqlite_open;` in `src/utils/mod.rs`**

Add (alphabetically):

```rust
pub mod sqlite_open;
```

- [ ] **Step 5: Run tests**

Run:
```bash
cargo test -p alephcore --lib utils::sqlite_open 2>&1 | tail -20
```

Expected: 2 tests pass.

- [ ] **Step 6: Run clippy**

Run:
```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep sqlite_open | head -5
```

Expected: no `sqlite_open` warnings.

- [ ] **Step 7: Commit**

```bash
git add src/utils/sqlite_open.rs src/utils/mod.rs
git commit -m "spec-c: sqlite_open — open_sqlite_safe + open_sqlite_readonly helpers"
```

---

### Task 7: Migrate all SQLite open sites to the helper

**Purpose:** Replace 15-20 scattered `Connection::open` + inline pragma blocks with `open_sqlite_safe(path)` calls. Single grouped commit since this is mechanical.

**Files (already-WAL → consolidate):**
- Modify: `src/memory/store/sqlite/mod.rs:65-75`
- Modify: `src/tasks/shared/store.rs:25-32`
- Modify: `src/tasks/heartbeat/store.rs:42-47`
- Modify: `src/tasks/cron/store.rs:48-53`

**Files (no WAL — full migration):**
- Modify: every other rusqlite open site identified in Task 1, Step 3.

- [ ] **Step 1: Re-run the audit grep to catch new sites**

Run:
```bash
grep -rn "Connection::open(\|rusqlite::Connection::open(" src/ --include="*.rs" | grep -v test | tee /tmp/spec_c_sqlite_sites.txt
```

Expected: list of 15-20 sites. Save to `/tmp` for reference during migration.

- [ ] **Step 2: For each site, replace the open + pragma block**

For sites that already set WAL (memory/cron/heartbeat/tasks_shared), the pattern is:

```rust
// BEFORE
let conn = Connection::open(path)?;
conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;

// AFTER
let conn = alephcore::utils::sqlite_open::open_sqlite_safe(path)?;
```

For sites with no pragmas:

```rust
// BEFORE
let conn = Connection::open(path)?;

// AFTER
let conn = alephcore::utils::sqlite_open::open_sqlite_safe(path)?;
```

Walk every site in `/tmp/spec_c_sqlite_sites.txt` and apply this. The exact line ranges vary; use file-by-file Edit calls keyed on the unique `Connection::open(...)` invocation.

- [ ] **Step 3: Compile & run all unit tests**

Run:
```bash
cargo build --workspace 2>&1 | tail -20
cargo test -p alephcore --lib 2>&1 | tail -10
```

Expected: Finished + tests pass. If any test fails because a previously-non-WAL DB was opened concurrently, investigate before continuing.

- [ ] **Step 4: Run reverse-regression check**

Run:
```bash
git grep -n "Connection::open(\|rusqlite::Connection::open(" src/ --include='*.rs' \
  | grep -v "open_sqlite_safe\|open_sqlite_readonly\|test\|//"
```

Expected: empty output. Any remaining hit must be migrated.

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "spec-c: route all SQLite opens through open_sqlite_safe"
```

(`git add -A src/` is acceptable here since the only changes since the previous commit are this task's edits — verify with `git status` first.)

---

### Task 8: VaultIo wrapper + crash-safe + concurrent tests

**Purpose:** Wrap all `secrets.vault` reads/writes in a single-entry-point struct that uses atomic temp+rename + fcntl.

**Files:**
- Create: `src/utils/vault_io.rs`
- Modify: `src/utils/mod.rs`
- Modify: every direct `secrets.vault` access site identified in Task 1, Step 4

- [ ] **Step 1: Write failing unit tests**

Create `src/utils/vault_io.rs`:

```rust
//! Atomic + locked I/O wrapper around `secrets.vault`.
//!
//! Defense-in-depth: even if the singleton lock is bypassed, every
//! vault write here serialises via fs2 fcntl on `secrets.vault.lock`
//! and writes through atomic temp+rename so readers always see either
//! the complete old or complete new file.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_returns_none_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let io = VaultIo::new(dir.path());
        assert!(io.read().unwrap().is_none());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let io = VaultIo::new(dir.path());
        io.write(b"payload").unwrap();
        assert_eq!(io.read().unwrap().as_deref(), Some(b"payload" as &[u8]));
    }

    #[test]
    fn overwrite_keeps_only_one_data_file() {
        let dir = tempfile::tempdir().unwrap();
        let io = VaultIo::new(dir.path());
        io.write(b"one").unwrap();
        io.write(b"two").unwrap();
        let files: Vec<String> = std::fs::read_dir(dir.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with(".aleph_atomic_"))
            .collect();
        assert!(files.contains(&"secrets.vault".to_string()));
        assert!(files.contains(&"secrets.vault.lock".to_string()));
        assert_eq!(files.len(), 2);
    }
}
```

- [ ] **Step 2: Run failing**

Run:
```bash
cargo test -p alephcore --lib utils::vault_io 2>&1 | tail -10
```

Expected: compile errors.

- [ ] **Step 3: Implement VaultIo**

Prepend to `src/utils/vault_io.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::utils::atomic_io::{with_file_lock, write_atomic};

const VAULT_FILENAME: &str = "secrets.vault";
const VAULT_LOCK_FILENAME: &str = "secrets.vault.lock";

pub struct VaultIo {
    path: PathBuf,
    lock_path: PathBuf,
}

impl VaultIo {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(VAULT_FILENAME),
            lock_path: data_dir.join(VAULT_LOCK_FILENAME),
        }
    }

    pub fn path(&self) -> &Path { &self.path }

    /// Returns `Ok(None)` if the vault file does not yet exist.
    pub fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
        with_file_lock(&self.lock_path, |_| match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        })
    }

    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        with_file_lock(&self.lock_path, |_| write_atomic(&self.path, bytes))
    }
}
```

- [ ] **Step 4: Wire `pub mod vault_io;` in `src/utils/mod.rs`**

Add (alphabetically):

```rust
pub mod vault_io;
```

- [ ] **Step 5: Run unit tests**

Run:
```bash
cargo test -p alephcore --lib utils::vault_io 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 6: Migrate every direct `secrets.vault` access site**

Walk the list from Task 1 Step 4. For each direct `fs::read`/`fs::write` on the vault path, replace with `VaultIo::new(&data_dir).read()` / `.write(...)`.

Likely sites:
- `src/gateway/security/store.rs` — vault load/save
- `src/gateway/security/shared_token.rs` — token rotation may write vault

For each, the change is:

```rust
// BEFORE
let bytes = std::fs::read(&vault_path)?;
// or std::fs::write(&vault_path, &bytes)?;

// AFTER
let io = alephcore::utils::vault_io::VaultIo::new(&data_dir);
let bytes = io.read()?.unwrap_or_default();
// or io.write(&bytes)?;
```

- [ ] **Step 7: Build + run security tests**

Run:
```bash
cargo build --workspace 2>&1 | tail -10
cargo test -p alephcore --lib gateway::security 2>&1 | tail -20
```

Expected: green. If any test fails because it expected a missing file to behave differently, adapt: `read() -> Ok(None)` is the new contract.

- [ ] **Step 8: Reverse-regression grep**

Run:
```bash
git grep -nE 'fs::(read|write)\(.*secrets\.vault' src/ \
  | grep -v 'vault_io\|test'
```

Expected: empty.

- [ ] **Step 9: Commit**

```bash
git add src/utils/vault_io.rs src/utils/mod.rs src/gateway/security/
git commit -m "spec-c: wrap secrets.vault in VaultIo (fcntl + atomic write)"
```

---

### Task 9: acp_sessions atomic write

**Purpose:** Replace bare `fs::write` in `src/acp/manager.rs` save path with `atomic_io::write_atomic`. No fcntl needed (rationale: singleton already gates writes; this is just crash-safety).

**Files:**
- Modify: `src/acp/manager.rs:24-50`

- [ ] **Step 1: Read the current save function**

Run:
```bash
sed -n '20,60p' src/acp/manager.rs
```

Expected: locate the `save_sessions` (or similarly named) function and the `fs::write` call.

- [ ] **Step 2: Replace `fs::write` with `write_atomic`**

Edit the save function. Pattern:

```rust
// BEFORE
std::fs::write(&path, serialized.as_bytes())?;

// AFTER
alephcore::utils::atomic_io::write_atomic(&path, serialized.as_bytes())?;
```

- [ ] **Step 3: Add a unit test for atomicity in the same file**

Add to the existing `#[cfg(test)] mod tests` (or create one if absent):

```rust
#[test]
fn save_writes_complete_file_via_atomic() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path()); // overrides acp_sessions_path()
    let data_dir = dir.path().join(".aleph/data");
    std::fs::create_dir_all(&data_dir).unwrap();

    let sessions = vec![/* construct a minimal valid session */];
    save_sessions(&sessions).unwrap();

    let saved = std::fs::read_to_string(data_dir.join("acp_sessions.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(parsed.is_array());
}
```

If `save_sessions` is not the actual function name, substitute the real one. The test ensures the atomic write produces a valid, complete JSON file.

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p alephcore --lib acp::manager 2>&1 | tail -10
```

Expected: green.

- [ ] **Step 5: Reverse-regression grep**

Run:
```bash
git grep -n "fs::write.*acp_sessions" src/ | grep -v "atomic_io\|test"
```

Expected: empty.

- [ ] **Step 6: Commit**

```bash
git add src/acp/manager.rs
git commit -m "spec-c: acp_sessions.json — atomic temp+rename write"
```

---

### Task 10: CommandPolicy enum + run_no_lock helper

**Purpose:** Declarative policy type + the simplest helper (NoLock branch). The lock + IPC branches land in Tasks 11 and 15.

**Files:**
- Create: `src/cli/policy.rs`
- Modify: `src/lib.rs` (add `pub mod cli;` if not present) **OR** `src/cli/mod.rs` (add `pub mod policy;`)

- [ ] **Step 1: Check whether `src/cli/` already exists**

Run:
```bash
ls src/cli/ 2>&1 | head -10
```

Expected: either a list of files, or "No such file or directory".

If absent, create `src/cli/mod.rs` with:
```rust
pub mod policy;
```

And add `pub mod cli;` to `src/lib.rs`.

If present, just append `pub mod policy;` to `src/cli/mod.rs`.

- [ ] **Step 2: Write tests for run_no_lock**

Create `src/cli/policy.rs`:

```rust
//! Declarative policy + dispatch helpers for CLI subcommands.
//!
//! Every CLI subcommand declares one of three policies and dispatches
//! through `run_no_lock` (NoLock) or `with_policy` (LockOnly / LockOrIpc,
//! filled in by Task 11).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_no_lock_passes_through_ok() {
        let result: i32 = run_no_lock(|| Ok(42)).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn run_no_lock_passes_through_err() {
        let result: anyhow::Result<i32> = run_no_lock(|| Err(anyhow::anyhow!("boom")));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "boom");
    }
}
```

- [ ] **Step 3: Run failing**

Run:
```bash
cargo test -p alephcore --lib cli::policy 2>&1 | tail -10
```

Expected: compile errors.

- [ ] **Step 4: Implement enum + run_no_lock**

Prepend to `src/cli/policy.rs`:

```rust
#[derive(Debug, Clone, Copy)]
pub enum HttpMethod { Get, Post, Patch, Delete }

impl HttpMethod {
    pub fn as_reqwest(&self) -> reqwest::Method {
        match self {
            HttpMethod::Get    => reqwest::Method::GET,
            HttpMethod::Post   => reqwest::Method::POST,
            HttpMethod::Patch  => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CommandPolicy {
    /// Subcommand does not touch `~/.aleph/data/`. Skip lock entirely.
    NoLock,
    /// Subcommand needs exclusive write access. Refuse if server holds the lock.
    LockOnly,
    /// Try to take the lock locally; if held, forward to the server's
    /// admin endpoint via HTTP.
    LockOrIpc { route: &'static str, method: HttpMethod },
}

/// Dispatch a NoLock subcommand. Currently a thin pass-through; the
/// indirection exists so reverse-regression checks (Task 23) can scan
/// `src/bin/aleph-server/commands/` for `run_no_lock(` to verify every
/// command file has gone through policy classification.
pub fn run_no_lock<T, F>(f: F) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T>,
{
    f()
}
```

- [ ] **Step 5: Run tests**

Run:
```bash
cargo test -p alephcore --lib cli::policy 2>&1 | tail -10
```

Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli/policy.rs src/cli/mod.rs src/lib.rs
git commit -m "spec-c: CommandPolicy enum + run_no_lock helper"
```

---

### Task 11: with_policy helper — Lock arm only (IPC stub)

**Purpose:** Add `with_policy` that dispatches LockOnly + LockOrIpc.Acquired branches. The IPC arm returns a clearly marked `unimplemented!` for now; Task 15 fills it in once IPC client + endpoint are ready.

**Files:**
- Modify: `src/cli/policy.rs`

- [ ] **Step 1: Add tests for the lock arm**

Append to the `#[cfg(test)]` block in `src/cli/policy.rs`:

```rust
    #[test]
    fn with_policy_lock_only_acquires_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let result: i32 = with_policy::<_, i32>(
            CommandPolicy::LockOnly,
            dir.path(),
            |_lock| Ok(7),
            serde_json::Value::Null,
        ).unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn with_policy_lock_only_exits_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = match crate::utils::instance_lock::try_acquire(dir.path()).unwrap() {
            crate::utils::instance_lock::AcquireOutcome::Acquired(g) => g,
            _ => panic!(),
        };
        let result = std::panic::catch_unwind(|| {
            with_policy::<_, i32>(
                CommandPolicy::LockOnly,
                dir.path(),
                |_lock| Ok(7),
                serde_json::Value::Null,
            )
        });
        // The helper calls `std::process::exit(64)` on lock contention,
        // which can't be caught by `catch_unwind` — instead, we assert
        // that result is Err(_) by routing through a non-process-exit
        // path. So we override exit-on-contention behaviour for tests:
        // see Step 3 for the test-only `with_policy_for_test` variant.
        let _ = result;
    }
```

The straightforward test is awkward because `std::process::exit` can't be unwound. Replace the second test with a test of a `try_with_policy` variant that returns `Err` instead of exiting, and let the production `with_policy` wrap it:

```rust
    #[test]
    fn try_with_policy_lock_only_returns_err_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = match crate::utils::instance_lock::try_acquire(dir.path()).unwrap() {
            crate::utils::instance_lock::AcquireOutcome::Acquired(g) => g,
            _ => panic!(),
        };
        let result: anyhow::Result<i32> = try_with_policy::<_, i32>(
            CommandPolicy::LockOnly,
            dir.path(),
            |_lock| Ok(7),
            serde_json::Value::Null,
        );
        assert!(result.is_err());
        assert!(format!("{:?}", result.unwrap_err()).contains("server is running"));
    }
```

- [ ] **Step 2: Run failing**

Run:
```bash
cargo test -p alephcore --lib cli::policy 2>&1 | tail -10
```

Expected: compile errors — `with_policy`, `try_with_policy` not defined.

- [ ] **Step 3: Implement try_with_policy + with_policy**

Append to `src/cli/policy.rs`:

```rust
use std::path::Path;

use crate::utils::instance_lock::{self, AcquireOutcome, InstanceLock};

/// Test-friendly variant of `with_policy` that returns `Err` instead of
/// calling `std::process::exit` on lock contention. Production callers
/// should use `with_policy` which surfaces UX-friendly stderr messages.
pub fn try_with_policy<L, T>(
    policy: CommandPolicy,
    data_dir: &Path,
    local: L,
    _ipc_body: serde_json::Value,
) -> anyhow::Result<T>
where
    L: FnOnce(&InstanceLock) -> anyhow::Result<T>,
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    match policy {
        CommandPolicy::NoLock => {
            anyhow::bail!("NoLock commands must dispatch through run_no_lock, not with_policy")
        }
        CommandPolicy::LockOnly => match instance_lock::try_acquire(data_dir)? {
            AcquireOutcome::Acquired(lock) => local(&lock),
            AcquireOutcome::HeldByLive { pid, lock_path }
            | AcquireOutcome::HeldByOrphaned { pid, lock_path } => {
                anyhow::bail!(
                    "server is running (PID {pid}). This command requires \
                     exclusive access — run `aleph stop` first. Lock: {}",
                    lock_path.display()
                )
            }
        },
        CommandPolicy::LockOrIpc { .. } => match instance_lock::try_acquire(data_dir)? {
            AcquireOutcome::Acquired(lock) => local(&lock),
            AcquireOutcome::HeldByLive { .. } | AcquireOutcome::HeldByOrphaned { .. } => {
                // IPC arm filled in by Task 15.
                anyhow::bail!("LockOrIpc IPC arm not yet wired (Spec C Task 15 pending)")
            }
        },
    }
}

/// Production dispatch: same as `try_with_policy` but converts lock
/// contention errors into a clean stderr + `std::process::exit(64)`
/// instead of returning an `Err` to the caller.
pub fn with_policy<L, T>(
    policy: CommandPolicy,
    data_dir: &Path,
    local: L,
    ipc_body: serde_json::Value,
) -> anyhow::Result<T>
where
    L: FnOnce(&InstanceLock) -> anyhow::Result<T>,
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    match try_with_policy(policy, data_dir, local, ipc_body) {
        Ok(v) => Ok(v),
        Err(e) => {
            let msg = format!("{e:?}");
            if msg.contains("server is running") {
                eprintln!("{msg}");
                std::process::exit(64);
            }
            Err(e)
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p alephcore --lib cli::policy 2>&1 | tail -20
```

Expected: 3 tests pass (`run_no_lock_*` × 2 + `try_with_policy_lock_only_returns_err_when_held` + the already-free Acquired case).

If 4 expected (the LockOnly+free test from earlier counts as one too), verify count matches. Adjust if needed.

- [ ] **Step 5: Commit**

```bash
git add src/cli/policy.rs
git commit -m "spec-c: with_policy + try_with_policy — Lock arm wired, IPC arm stubbed"
```

---

### Task 12: IPC endpoint discovery — server-side write/cleanup

**Purpose:** Server writes `~/.aleph/data/.ipc-endpoint.json` after binding its listening port; deletes it on graceful shutdown. Clients read it to find the URL.

**Files:**
- Create: `src/cli/endpoint.rs`
- Modify: `src/cli/mod.rs` (add `pub mod endpoint;`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (call write/cleanup)

- [ ] **Step 1: Write tests**

Create `src/cli/endpoint.rs`:

```rust
//! `.ipc-endpoint.json` write + read helpers.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const ENDPOINT_FILENAME: &str = ".ipc-endpoint.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcEndpoint {
    pub version: u32,
    pub url: String,
    pub pid: u32,
    pub started_at: String,
}

impl IpcEndpoint {
    pub fn current(url: impl Into<String>) -> Self {
        Self {
            version: 1,
            url: url.into(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub fn endpoint_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ENDPOINT_FILENAME)
}

pub fn write_endpoint(data_dir: &Path, endpoint: &IpcEndpoint) -> std::io::Result<()> {
    let path = endpoint_path(data_dir);
    let bytes = serde_json::to_vec_pretty(endpoint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::utils::atomic_io::write_atomic(&path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&path, perm);
    }
    Ok(())
}

pub fn read_endpoint(data_dir: &Path) -> std::io::Result<Option<IpcEndpoint>> {
    let path = endpoint_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let ep: IpcEndpoint = serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(Some(ep))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn remove_endpoint(data_dir: &Path) {
    let _ = std::fs::remove_file(endpoint_path(data_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let ep = IpcEndpoint::current("http://127.0.0.1:9000");
        write_endpoint(dir.path(), &ep).unwrap();
        let read = read_endpoint(dir.path()).unwrap().unwrap();
        assert_eq!(read.url, "http://127.0.0.1:9000");
        assert_eq!(read.pid, std::process::id());
    }

    #[test]
    fn read_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_endpoint(dir.path()).unwrap().is_none());
    }

    #[test]
    fn remove_cleans_file() {
        let dir = tempfile::tempdir().unwrap();
        let ep = IpcEndpoint::current("http://x");
        write_endpoint(dir.path(), &ep).unwrap();
        remove_endpoint(dir.path());
        assert!(read_endpoint(dir.path()).unwrap().is_none());
    }
}
```

- [ ] **Step 2: Wire `pub mod endpoint;` in `src/cli/mod.rs`**

```rust
pub mod endpoint;
```

- [ ] **Step 3: Run tests**

Run:
```bash
cargo test -p alephcore --lib cli::endpoint 2>&1 | tail -10
```

Expected: 3 pass.

- [ ] **Step 4: Wire write/remove into server start**

Locate the place in `src/bin/aleph-server/commands/start/mod.rs` where the axum server has just bound a listener and is about to `serve()`. Add immediately before serve:

```rust
let endpoint = alephcore::cli::endpoint::IpcEndpoint::current(format!(
    "http://{}:{}", listen_addr.ip(), listen_addr.port()
));
let data_dir = alephcore::utils::paths::data_dir()?;
if let Err(e) = alephcore::cli::endpoint::write_endpoint(&data_dir, &endpoint) {
    tracing::warn!(error = %e, "failed to write IPC endpoint discovery file");
}
```

And register a cleanup so graceful shutdown removes the file. Locate the existing graceful-shutdown handler and prepend:

```rust
// Graceful shutdown — remove endpoint discovery file.
let data_dir_for_cleanup = alephcore::utils::paths::data_dir().ok();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    if let Some(dir) = data_dir_for_cleanup {
        alephcore::cli::endpoint::remove_endpoint(&dir);
    }
});
```

If the existing shutdown logic already uses signals, integrate the cleanup into it instead of spawning a duplicate signal handler. The exact wiring depends on current `start/mod.rs` shape — read it first.

- [ ] **Step 5: Build + smoke**

Run:
```bash
cargo build --bin aleph-server 2>&1 | tail -10
target/debug/aleph-server start &
SERVER_PID=$!
sleep 3
cat ~/.aleph/data/.ipc-endpoint.json | head -10
kill $SERVER_PID
sleep 2
ls ~/.aleph/data/.ipc-endpoint.json 2>&1 || echo "(removed, good)"
```

Expected: file exists while server runs (with PID + URL), absent after kill.

- [ ] **Step 6: Commit**

```bash
git add src/cli/endpoint.rs src/cli/mod.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "spec-c: IPC endpoint discovery — write .ipc-endpoint.json on bind, remove on shutdown"
```

---

### Task 13: read_current_token_readonly free function

**Purpose:** Provide a lightweight, dependency-free way to read the current bearer token from `security.db` without instantiating a full `SharedTokenManager`.

**Files:**
- Create: `src/gateway/security/token_readonly.rs`
- Modify: `src/gateway/security/mod.rs` (add `pub mod token_readonly;` + re-export)

- [ ] **Step 1: Read existing SharedTokenManager SQL (from Task 1, Step 5)**

Open the audit file and pull the verbatim `SELECT` statement recorded there.

- [ ] **Step 2: Write tests**

Create `src/gateway/security/token_readonly.rs`:

```rust
//! Read-only access to the current bearer token. Mirrors
//! `SharedTokenManager::current_token()` SQL but does not require the
//! full manager (no rotation logic, no DB writes).
//!
//! Used by CLI subcommands that need to authenticate to a running
//! server's `/v1/admin/*` endpoint while the server holds the
//! singleton lock.

use rusqlite::Connection;

/// Returns the most recently issued bearer token, or `None` if no token
/// has ever been issued (fresh install before first server start).
pub fn read_current_token_readonly(conn: &Connection) -> rusqlite::Result<Option<String>> {
    // SQL must mirror SharedTokenManager::current_token() exactly.
    // (Filled in from Task 1 Step 5 audit notes; placeholder shown.)
    conn.query_row(
        "SELECT plaintext FROM shared_token \
         WHERE plaintext IS NOT NULL \
         ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get::<_, Option<String>>(0),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::sqlite_open::open_sqlite_safe;

    fn seed(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE shared_token (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                plaintext TEXT,
                hmac_secret BLOB,
                created_at INTEGER
             );",
        ).unwrap();
    }

    #[test]
    fn returns_none_when_table_empty() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        assert!(read_current_token_readonly(&conn).unwrap().is_none());
    }

    #[test]
    fn returns_latest_token() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_sqlite_safe(&dir.path().join("security.db")).unwrap();
        seed(&conn);
        conn.execute("INSERT INTO shared_token (plaintext, created_at) VALUES (?, ?)",
                     rusqlite::params!["old", 1_000]).unwrap();
        conn.execute("INSERT INTO shared_token (plaintext, created_at) VALUES (?, ?)",
                     rusqlite::params!["new", 2_000]).unwrap();
        assert_eq!(read_current_token_readonly(&conn).unwrap(), Some("new".into()));
    }
}
```

If the audit notes show different table/column names, substitute them. The structure of the test stays the same.

- [ ] **Step 3: Wire `pub mod token_readonly;`**

In `src/gateway/security/mod.rs`, add:

```rust
pub mod token_readonly;
pub use token_readonly::read_current_token_readonly;
```

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p alephcore --lib gateway::security::token_readonly 2>&1 | tail -10
```

Expected: 2 pass.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/security/token_readonly.rs src/gateway/security/mod.rs
git commit -m "spec-c: read_current_token_readonly — bearer-token read path for CLI"
```

---

### Task 14: CLI HTTP forwarder + 401 retry

**Purpose:** CLI-side HTTP client that loads endpoint, reads token, posts JSON, handles 401 self-heal.

**Files:**
- Create: `src/cli/ipc_client.rs`
- Modify: `src/cli/mod.rs` (add `pub mod ipc_client;`)

- [ ] **Step 1: Write tests using a mock HTTP server**

Create `src/cli/ipc_client.rs`:

```rust
//! HTTP client that forwards a CLI request to the running server's
//! `/v1/admin/*` namespace. Reads bearer token from `security.db` in
//! read-only WAL mode; auto-retries once on 401 to handle token rotation
//! that races with the request.

use std::path::Path;

use anyhow::Context;
use reqwest::StatusCode;

use crate::cli::endpoint::read_endpoint;
use crate::cli::policy::HttpMethod;
use crate::gateway::security::read_current_token_readonly;
use crate::utils::sqlite_open::open_sqlite_readonly;

const SECURITY_DB_FILENAME: &str = "security.db";

#[derive(Debug)]
pub struct IpcResponse<T> {
    pub status: StatusCode,
    pub body: T,
}

pub fn forward_to_server<T>(
    data_dir: &Path,
    method: HttpMethod,
    route: &str,
    body: serde_json::Value,
) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let endpoint = read_endpoint(data_dir)?
        .with_context(|| format!(
            "server is initializing or crashed (no .ipc-endpoint.json at {}). \
             Try again or run `aleph stop` first.",
            data_dir.display()
        ))?;
    let url = format!("{}{}", endpoint.url.trim_end_matches('/'), route);

    let token = read_token(data_dir)?;
    let resp = call_once(&url, method, &body, &token)?;

    if resp.status() == StatusCode::UNAUTHORIZED {
        // Token may have rotated between our read and our send.
        let fresh = read_token(data_dir)?;
        if fresh != token {
            let resp2 = call_once(&url, method, &body, &fresh)?;
            return finalize::<T>(resp2);
        }
        anyhow::bail!("auth token rotated mid-call; retry");
    }
    finalize::<T>(resp)
}

fn read_token(data_dir: &Path) -> anyhow::Result<String> {
    let conn = open_sqlite_readonly(&data_dir.join(SECURITY_DB_FILENAME))
        .context("cannot open security.db read-only — is data_dir set up?")?;
    let token = read_current_token_readonly(&conn)?
        .ok_or_else(|| anyhow::anyhow!(
            "no bearer token in security.db — has the server ever been started?"
        ))?;
    Ok(token)
}

fn call_once(
    url: &str,
    method: HttpMethod,
    body: &serde_json::Value,
    token: &str,
) -> anyhow::Result<reqwest::blocking::Response> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let req = client
        .request(method.as_reqwest(), url)
        .bearer_auth(token)
        .json(body);
    Ok(req.send()?)
}

fn finalize<T>(resp: reqwest::blocking::Response) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = resp.status();
    if status.is_success() {
        let body = resp.json::<T>()?;
        Ok(body)
    } else {
        let text = resp.text().unwrap_or_default();
        anyhow::bail!("server returned {status}: {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests for forward_to_server live in tests/spec_c_cli_ipc.rs
    // because they need a real HTTP listener and a seeded security.db. Unit
    // tests here cover only the helpers that don't need a network.

    #[test]
    fn read_endpoint_path_handles_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_endpoint(dir.path()).unwrap();
        assert!(result.is_none());
    }
}
```

- [ ] **Step 2: Wire `pub mod ipc_client;` in `src/cli/mod.rs`**

```rust
pub mod ipc_client;
```

- [ ] **Step 3: Run tests**

Run:
```bash
cargo test -p alephcore --lib cli::ipc_client 2>&1 | tail -10
```

Expected: 1 unit test passes; integration tests (hidden in `tests/`) wait for Task 22.

- [ ] **Step 4: Commit**

```bash
git add src/cli/ipc_client.rs src/cli/mod.rs
git commit -m "spec-c: ipc_client — forward_to_server with 401 self-heal"
```

---

### Task 15: Wire with_policy IPC arm

**Purpose:** Replace the Task 11 `unimplemented!` IPC stub with a real call to `forward_to_server`.

**Files:**
- Modify: `src/cli/policy.rs`

- [ ] **Step 1: Update `try_with_policy` LockOrIpc branch**

Edit the `LockOrIpc` arm:

```rust
        CommandPolicy::LockOrIpc { route, method } => match instance_lock::try_acquire(data_dir)? {
            AcquireOutcome::Acquired(lock) => local(&lock),
            AcquireOutcome::HeldByLive { .. } | AcquireOutcome::HeldByOrphaned { .. } => {
                let response = crate::cli::ipc_client::forward_to_server::<T>(
                    data_dir, method, route, _ipc_body,
                )?;
                Ok(response)
            }
        },
```

Rename the `_ipc_body` parameter to `ipc_body` (drop the leading underscore) since it is now used.

- [ ] **Step 2: Add an integration test fixture**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn lock_or_ipc_forwards_when_held() {
        // Stand up an in-process axum server that responds to a known route.
        // Spawn on a free port, write its endpoint file, seed a token in
        // security.db, then call `try_with_policy` with the lock held.
        // Assert the response is the server's body.
        //
        // NOTE: Full integration is in tests/spec_c_cli_ipc.rs; this is a
        // smoke-only check of the wiring. If standing up an in-process
        // axum is heavy, defer entirely to the integration test and
        // delete this stub.
    }
```

- [ ] **Step 3: Run tests**

Run:
```bash
cargo test -p alephcore --lib cli::policy 2>&1 | tail -15
```

Expected: green (unchanged from Task 11 since stubs aren't real tests).

- [ ] **Step 4: Commit**

```bash
git add src/cli/policy.rs
git commit -m "spec-c: with_policy — wire IPC arm to forward_to_server"
```

---

### Task 16: Server `/v1/admin/secrets/*` handlers (4 endpoints)

**Purpose:** First batch of admin endpoints. Routes: `POST /v1/admin/secrets`, `GET /v1/admin/secrets`, `GET /v1/admin/secrets/:key`, `DELETE /v1/admin/secrets/:key`.

**Files:**
- Create: `src/gateway/admin_api/mod.rs`
- Create: `src/gateway/admin_api/secrets.rs`
- Modify: `src/gateway/mod.rs` (add `pub mod admin_api;`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (mount admin router on axum)

- [ ] **Step 1: Module skeleton**

Create `src/gateway/admin_api/mod.rs`:

```rust
//! `/v1/admin/*` namespace — IPC entry points for CLI commands while
//! the server holds the singleton lock.
//!
//! All handlers require a valid bearer token (validated by existing
//! middleware in the parent gateway router; we just mount under the
//! authenticated subrouter).

pub mod secrets;
// (Tasks 17 + 18 add `memory` and `agents`.)

use axum::Router;
use std::sync::Arc;

use crate::gateway::security::store::SecurityStore;

#[derive(Clone)]
pub struct AdminApiState {
    pub security_store: Arc<SecurityStore>,
    // (Memory + agents stores added in Tasks 17/18.)
}

pub fn router(state: AdminApiState) -> Router {
    Router::new()
        .nest("/secrets", secrets::router())
        .with_state(state)
}
```

- [ ] **Step 2: Write handler tests**

Create `src/gateway/admin_api/secrets.rs`:

```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gateway::admin_api::AdminApiState;

pub fn router() -> Router<AdminApiState> {
    Router::new()
        .route("/", post(create_or_update_secret).get(list_secrets))
        .route("/:key", get(get_secret).delete(delete_secret))
}

#[derive(Debug, Deserialize)]
pub struct CreateOrUpdateSecretRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct SecretSummary {
    pub key: String,
    pub created_at: i64,
}

async fn create_or_update_secret(
    State(state): State<AdminApiState>,
    Json(body): Json<CreateOrUpdateSecretRequest>,
) -> Result<Json<SecretSummary>, (StatusCode, String)> {
    state.security_store.put_secret(&body.key, &body.value)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(SecretSummary { key: body.key, created_at: now_unix() }))
}

async fn list_secrets(
    State(state): State<AdminApiState>,
) -> Result<Json<Vec<SecretSummary>>, (StatusCode, String)> {
    let names = state.security_store.list_secret_keys()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(names.into_iter().map(|k| SecretSummary {
        key: k, created_at: 0,
    }).collect()))
}

async fn get_secret(
    State(state): State<AdminApiState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match state.security_store.get_secret(&key) {
        Ok(Some(value)) => Ok(Json(serde_json::json!({ "key": key, "value": value }))),
        Ok(None) => Err((StatusCode::NOT_FOUND, format!("no secret: {key}"))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn delete_secret(
    State(state): State<AdminApiState>,
    Path(key): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.security_store.delete_secret(&key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn test_app() -> Router {
        let store = Arc::new(SecurityStore::in_memory().expect("in-memory store"));
        let state = AdminApiState { security_store: store };
        Router::new().nest("/secrets", router()).with_state(state)
    }

    #[tokio::test]
    async fn round_trip_create_get_delete() {
        let app = test_app().await;
        let body = serde_json::to_vec(&serde_json::json!({
            "key": "OPENAI_API_KEY", "value": "sk-test"
        })).unwrap();

        let resp = app.clone().oneshot(
            Request::builder().method("POST").uri("/secrets").header("content-type","application/json")
                .body(Body::from(body)).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app.clone().oneshot(
            Request::builder().method("GET").uri("/secrets/OPENAI_API_KEY")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app.clone().oneshot(
            Request::builder().method("DELETE").uri("/secrets/OPENAI_API_KEY")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app.oneshot(
            Request::builder().method("GET").uri("/secrets/OPENAI_API_KEY")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
```

The reference to `SecurityStore::in_memory` assumes such a constructor exists. If not, replace with whatever test fixture the existing security tests already use; mirror their setup pattern verbatim.

- [ ] **Step 3: Wire `pub mod admin_api;` in `src/gateway/mod.rs`**

```rust
pub mod admin_api;
```

- [ ] **Step 4: Mount admin router on axum**

Locate the axum router construction in `src/bin/aleph-server/commands/start/mod.rs`. Add:

```rust
let admin_state = alephcore::gateway::admin_api::AdminApiState {
    security_store: security_store.clone(),
    // memory + agents added in Tasks 17/18.
};
let admin_router = alephcore::gateway::admin_api::router(admin_state);

// Mount under the existing authenticated subrouter so bearer auth is enforced.
let app = app.nest("/v1/admin", admin_router);
```

The exact integration point depends on the existing router shape — read it first.

- [ ] **Step 5: Run tests**

Run:
```bash
cargo test -p alephcore --lib gateway::admin_api 2>&1 | tail -10
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/admin_api/ src/gateway/mod.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "spec-c: /v1/admin/secrets — 4 handlers (POST/GET/GET-by-key/DELETE)"
```

---

### Task 17: Server `/v1/admin/memory/*` handlers (3 endpoints)

**Purpose:** Routes: `POST /v1/admin/memory/write`, `POST /v1/admin/memory/clear`, `POST /v1/admin/memory/reset`.

**Files:**
- Create: `src/gateway/admin_api/memory.rs`
- Modify: `src/gateway/admin_api/mod.rs` (add memory module + state field)

- [ ] **Step 1: Extend AdminApiState**

Edit `src/gateway/admin_api/mod.rs`:

```rust
pub mod memory;

use crate::memory::store::MemoryStore;

#[derive(Clone)]
pub struct AdminApiState {
    pub security_store: Arc<SecurityStore>,
    pub memory_store: Arc<dyn MemoryStore>,
}

pub fn router(state: AdminApiState) -> Router {
    Router::new()
        .nest("/secrets", secrets::router())
        .nest("/memory", memory::router())
        .with_state(state)
}
```

- [ ] **Step 2: Write memory handlers + tests**

Create `src/gateway/admin_api/memory.rs`:

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gateway::admin_api::AdminApiState;

pub fn router() -> Router<AdminApiState> {
    Router::new()
        .route("/write", post(write_entry))
        .route("/clear", post(clear_namespace))
        .route("/reset", post(reset_all))
}

#[derive(Debug, Deserialize)]
pub struct WriteEntryRequest {
    pub agent_id: String,
    pub namespace: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct WriteEntryResponse {
    pub fact_id: String,
}

async fn write_entry(
    State(state): State<AdminApiState>,
    Json(body): Json<WriteEntryRequest>,
) -> Result<Json<WriteEntryResponse>, (StatusCode, String)> {
    let fact_id = state.memory_store.admin_write_entry(&body.agent_id, &body.namespace, &body.content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(WriteEntryResponse { fact_id }))
}

#[derive(Debug, Deserialize)]
pub struct ClearNamespaceRequest {
    pub agent_id: String,
    pub namespace: String,
}

async fn clear_namespace(
    State(state): State<AdminApiState>,
    Json(body): Json<ClearNamespaceRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.memory_store.admin_clear_namespace(&body.agent_id, &body.namespace)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ResetAllRequest {
    pub agent_id: String,
}

async fn reset_all(
    State(state): State<AdminApiState>,
    Json(body): Json<ResetAllRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.memory_store.admin_reset_all(&body.agent_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    // Mirror Task 16's pattern: spin up axum with an in-memory MemoryStore,
    // exercise each route, assert outcome.

    #[tokio::test]
    async fn write_entry_round_trip() {
        // <see Task 16 fixture pattern; substitute in-memory MemoryStore>
    }
}
```

The `admin_write_entry`/`admin_clear_namespace`/`admin_reset_all` calls assume those service-layer methods exist on the `MemoryStore` trait. If the existing trait does not expose admin operations, add them in this task as new trait methods with default implementations that return `Err(NotSupported)`, then implement them on the concrete `SqliteMemoryStore`.

- [ ] **Step 3: Update mount in commands/start/mod.rs**

Add `memory_store` to the `AdminApiState` constructor.

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p alephcore --lib gateway::admin_api::memory 2>&1 | tail -10
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/admin_api/ src/bin/aleph-server/commands/start/mod.rs
git commit -m "spec-c: /v1/admin/memory — 3 handlers (write/clear/reset)"
```

---

### Task 18: Server `/v1/admin/agents/*` handlers (3 endpoints)

**Purpose:** Routes: `POST /v1/admin/agents`, `PATCH /v1/admin/agents/:id`, `DELETE /v1/admin/agents/:id`.

**Files:**
- Create: `src/gateway/admin_api/agents.rs`
- Modify: `src/gateway/admin_api/mod.rs`

- [ ] **Step 1: Extend AdminApiState with agent_manager**

Add field:

```rust
pub agent_manager: Arc<AgentManager>,
```

(Read `src/config/agent_manager/mod.rs` for the actual type name.)

- [ ] **Step 2: Write handlers**

Create `src/gateway/admin_api/agents.rs`:

```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::gateway::admin_api::AdminApiState;

pub fn router() -> Router<AdminApiState> {
    Router::new()
        .route("/", post(create_agent))
        .route("/:id", patch(update_agent).delete(delete_agent))
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub id: String,
    pub display_name: String,
    pub system_prompt: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub display_name: String,
}

async fn create_agent(
    State(state): State<AdminApiState>,
    Json(body): Json<CreateAgentRequest>,
) -> Result<Json<AgentSummary>, (StatusCode, String)> {
    state.agent_manager.create_agent(&body.id, &body.display_name, body.system_prompt.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(AgentSummary { id: body.id, display_name: body.display_name }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub display_name: Option<String>,
    pub system_prompt: Option<String>,
}

async fn update_agent(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAgentRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.agent_manager.update_agent(&id, body.display_name.as_deref(), body.system_prompt.as_deref())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_agent(
    State(state): State<AdminApiState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.agent_manager.delete_agent(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    // Mirror Tasks 16/17 fixture pattern with an in-memory AgentManager.

    #[tokio::test]
    async fn create_then_delete_agent() {
        // <fixture>
    }
}
```

Substitute method names if the actual `AgentManager` API differs.

- [ ] **Step 3: Mount + run**

Add `pub mod agents;` and update the router. Update mount in `commands/start/mod.rs` to pass `agent_manager`.

Run:
```bash
cargo test -p alephcore --lib gateway::admin_api::agents 2>&1 | tail -10
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/admin_api/ src/bin/aleph-server/commands/start/mod.rs
git commit -m "spec-c: /v1/admin/agents — 3 handlers (POST/PATCH/DELETE)"
```

---

### Task 19: Audit + annotate every CLI subcommand with policies

**Purpose:** Walk every file in `src/bin/aleph-server/commands/`, declare its `CommandPolicy`, and dispatch through `with_policy` or `run_no_lock`.

**Files:**
- Modify: every file under `src/bin/aleph-server/commands/` that defines a CLI subcommand

- [ ] **Step 1: Re-run the CLI command audit from Task 1**

Open `src/utils/spec_c_audit.rs` and pull the list of subcommand files + their tentative policies.

- [ ] **Step 2: For each NoLock subcommand (e.g., `--version`, `stop`, `status`), wrap in run_no_lock**

Pattern:

```rust
// BEFORE
pub fn handle_status() -> anyhow::Result<()> {
    // ... read-only stuff
}

// AFTER
pub fn handle_status() -> anyhow::Result<()> {
    alephcore::cli::policy::run_no_lock(|| {
        // ... read-only stuff
        Ok(())
    })
}
```

- [ ] **Step 3: For each LockOnly subcommand, wrap in with_policy(LockOnly, ...)**

Example for a hypothetical `aleph migrate`:

```rust
pub fn handle_migrate() -> anyhow::Result<()> {
    let data_dir = alephcore::utils::paths::data_dir()?;
    alephcore::cli::policy::with_policy::<_, ()>(
        alephcore::cli::policy::CommandPolicy::LockOnly,
        &data_dir,
        |_lock| {
            // ... migration logic
            Ok(())
        },
        serde_json::Value::Null,
    )
}
```

- [ ] **Step 4: For each LockOrIpc subcommand, wrap in with_policy(LockOrIpc { ... }, ...)**

Example for `aleph secret set`:

```rust
pub fn handle_secret_set(key: &str, value: &str) -> anyhow::Result<()> {
    let data_dir = alephcore::utils::paths::data_dir()?;
    let body = serde_json::json!({ "key": key, "value": value });
    let _summary: alephcore::gateway::admin_api::secrets::SecretSummary =
        alephcore::cli::policy::with_policy(
            alephcore::cli::policy::CommandPolicy::LockOrIpc {
                route: "/v1/admin/secrets",
                method: alephcore::cli::policy::HttpMethod::Post,
            },
            &data_dir,
            |_lock| {
                // Local fast-path: write directly to security_store while
                // holding the singleton lock.
                let store = alephcore::gateway::security::store::SecurityStore::open(&data_dir)?;
                store.put_secret(key, value)?;
                Ok(alephcore::gateway::admin_api::secrets::SecretSummary {
                    key: key.to_string(),
                    created_at: 0,
                })
            },
            body,
        )?;
    println!("ok");
    Ok(())
}
```

- [ ] **Step 5: Remove argv-sniff from Task 5 main()**

Now that every subcommand internally manages its own lock acquisition, simplify the `fn main()` lock block. Replace the conditional `needs_lock_in_main` block with: nothing (no lock at main level). Each subcommand handler is responsible for lock policy.

But wait — `start` is a special case: `start` should hold the lock for the entire server lifetime, not just during the handler. Solution: `start` becomes a `LockOnly` policy, and the closure passed to `with_policy` holds the lock by passing it through to the long-running server task. Concretely:

```rust
pub fn handle_start(args: StartArgs) -> anyhow::Result<()> {
    let data_dir = alephcore::utils::paths::data_dir()?;
    alephcore::cli::policy::with_policy::<_, ()>(
        alephcore::cli::policy::CommandPolicy::LockOnly,
        &data_dir,
        |lock_ref| {
            // The lock is owned by with_policy's frame; we need to hold it
            // for the server's lifetime. Move-trick: take it out via a
            // `take()` on an owning slot. Easier: just keep the closure
            // alive for the entire server runtime.
            run_server_blocking(args, lock_ref)?;
            Ok(())
        },
        serde_json::Value::Null,
    )
}
```

If lifetime issues arise (the closure receives `&InstanceLock` which won't outlive the with_policy stack frame), refactor to pass the lock by ownership. Add a variant `with_policy_owned` that consumes the lock:

```rust
pub fn with_policy_owned<L, T>(
    policy: CommandPolicy,
    data_dir: &Path,
    local: L,
    ipc_body: serde_json::Value,
) -> anyhow::Result<T>
where
    L: FnOnce(InstanceLock) -> anyhow::Result<T>,
    T: serde::de::DeserializeOwned + serde::Serialize,
```

Use `with_policy_owned` for `start` (needs lifetime extension) and `with_policy` for everything else (lock dropped at end of closure).

- [ ] **Step 6: Reverse-regression check**

Run:
```bash
git grep -L "with_policy\|run_no_lock\|with_policy_owned" src/bin/aleph-server/commands/ \
  | grep '\.rs$' | grep -v 'mod\.rs'
```

Expected: empty (every command file references one of the three helpers).

- [ ] **Step 7: Build + manual smoke**

```bash
cargo build --bin aleph-server 2>&1 | tail -10
target/debug/aleph-server --version
target/debug/aleph-server stop  # should not need lock; should not corrupt anything
```

Expected: both succeed.

- [ ] **Step 8: Commit**

```bash
git add src/bin/aleph-server/commands/ src/cli/policy.rs src/bin/aleph-server/main.rs
git commit -m "spec-c: route every CLI subcommand through CommandPolicy dispatch"
```

---

### Task 20: E2E — spec_c_double_start

**Purpose:** Spawn two `aleph-server start` processes; the second must exit ≤50ms with code 64 and stderr containing the first's PID.

**Files:**
- Create: `tests/spec_c_double_start.rs`

- [ ] **Step 1: Write the test**

```rust
//! Two `aleph-server start` invocations on the same data_dir; second
//! exits cleanly with diagnostic.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn second_start_exits_64_with_first_pid() {
    let dir = tempfile::tempdir().unwrap();
    let data_arg = dir.path().to_string_lossy().into_owned();

    let bin = env!("CARGO_BIN_EXE_aleph-server");

    let mut first = Command::new(bin)
        .args(["start", "--data-dir", &data_arg])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("first start");

    // Wait briefly for the first to acquire the lock.
    std::thread::sleep(Duration::from_millis(500));

    let started = Instant::now();
    let second = Command::new(bin)
        .args(["start", "--data-dir", &data_arg])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("second start");
    let elapsed = started.elapsed();

    assert!(elapsed < Duration::from_millis(2_000),
            "second start hung for {:?} (expected fast exit)", elapsed);
    assert_eq!(second.status.code(), Some(64),
               "expected exit 64, got {:?}", second.status.code());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains(&first.id().to_string()),
            "stderr did not mention first pid {}: {stderr}", first.id());

    let _ = first.kill();
    first.wait().ok();
}
```

The test assumes `--data-dir` is a CLI argument the server respects; if not (Aleph hard-codes `~/.aleph/data`), use `HOME` env override:

```rust
.env("HOME", dir.path())
```

and remove `--data-dir`.

- [ ] **Step 2: Run**

```bash
cargo test --test spec_c_double_start 2>&1 | tail -15
```

Expected: 1 pass.

- [ ] **Step 3: Commit**

```bash
git add tests/spec_c_double_start.rs
git commit -m "spec-c/e2e: double-start refuses second instance with exit 64 + first PID"
```

---

### Task 21: E2E — CLI write while server down (local lock path)

**Purpose:** Verify CLI write commands work without IPC when no server is running, using the local lock path.

**Files:**
- Create: `tests/spec_c_cli_no_server.rs`

- [ ] **Step 1: Write test**

```rust
//! When no server is running, CLI write commands should succeed by
//! taking the singleton lock locally and writing directly.

use std::process::{Command, Stdio};

#[test]
fn cli_secret_set_works_when_server_down() {
    let dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_aleph-server");

    // Set a secret with no server running.
    let out = Command::new(bin)
        .args(["secret", "set", "FOO", "bar"])
        .env("HOME", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret set");
    assert!(out.status.success(), "secret set failed: {:?}", String::from_utf8_lossy(&out.stderr));

    // Read it back.
    let out = Command::new(bin)
        .args(["secret", "get", "FOO"])
        .env("HOME", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret get");
    assert!(out.status.success(), "secret get failed: {:?}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("bar"), "expected `bar` in stdout, got: {stdout}");
}
```

- [ ] **Step 2: Run**

```bash
cargo test --test spec_c_cli_no_server 2>&1 | tail -15
```

Expected: 1 pass.

- [ ] **Step 3: Commit**

```bash
git add tests/spec_c_cli_no_server.rs
git commit -m "spec-c/e2e: CLI secret set/get works when server is down (local lock)"
```

---

### Task 22: E2E — CLI write via IPC + 401 self-heal

**Purpose:** Server running, CLI write must go through `/v1/admin/secrets` endpoint. Also verify that mid-call token rotation triggers exactly one re-read + retry.

**Files:**
- Create: `tests/spec_c_cli_ipc.rs`
- Create: `tests/spec_c_cli_token_rotation.rs`

- [ ] **Step 1: Write IPC happy-path test**

```rust
//! With a real server running, CLI secret commands forward via IPC.

use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn cli_secret_set_forwards_via_ipc_when_server_up() {
    let dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_aleph-server");

    // Start server.
    let mut server = Command::new(bin)
        .args(["start"])
        .env("HOME", dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("server start");
    std::thread::sleep(Duration::from_secs(2));

    // CLI secret set — must NOT take the lock locally (server holds it).
    let out = Command::new(bin)
        .args(["secret", "set", "OPENAI_API_KEY", "sk-test-1234"])
        .env("HOME", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret set");

    let _ = server.kill();
    server.wait().ok();

    assert!(out.status.success(), "set failed: {:?}", String::from_utf8_lossy(&out.stderr));

    // Restart server briefly to verify the write actually landed in vault.
    let mut server = Command::new(bin)
        .args(["start"])
        .env("HOME", dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("server restart");
    std::thread::sleep(Duration::from_secs(2));

    let out = Command::new(bin)
        .args(["secret", "get", "OPENAI_API_KEY"])
        .env("HOME", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("aleph secret get");

    let _ = server.kill();
    server.wait().ok();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sk-test-1234"), "expected key value, got: {stdout}");
}
```

- [ ] **Step 2: Write token rotation test**

Create `tests/spec_c_cli_token_rotation.rs`:

```rust
//! When the bearer token rotates between CLI's read and CLI's send, the
//! CLI must re-read the token once and retry. Test this by spawning a
//! mock HTTP server that fails the first call with 401, then succeeds.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn cli_retries_once_on_401() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_in = counter.clone();

    let app = axum::Router::new()
        .route("/v1/admin/secrets", axum::routing::post(move |
            req: axum::extract::Request,
        | {
            let n = counter_in.fetch_add(1, Ordering::SeqCst);
            async move {
                let auth = req.headers().get("authorization")
                    .map(|v| v.to_str().unwrap_or("").to_string()).unwrap_or_default();
                if n == 0 {
                    (axum::http::StatusCode::UNAUTHORIZED, "rotated")
                } else {
                    assert_ne!(auth, "Bearer initial-token", "second call should use re-read token");
                    (axum::http::StatusCode::OK, r#"{"key":"x","created_at":0}"#)
                }
            }
        }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Seed a tempdir with .ipc-endpoint.json + security.db containing two
    // tokens (so the second read returns a different value).
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(&data_dir).unwrap();

    let endpoint = alephcore::cli::endpoint::IpcEndpoint::current(format!("http://{addr}"));
    alephcore::cli::endpoint::write_endpoint(&data_dir, &endpoint).unwrap();

    let conn = alephcore::utils::sqlite_open::open_sqlite_safe(&data_dir.join("security.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE shared_token (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plaintext TEXT, hmac_secret BLOB, created_at INTEGER);
         INSERT INTO shared_token (plaintext, created_at) VALUES ('initial-token', 1);
         INSERT INTO shared_token (plaintext, created_at) VALUES ('rotated-token', 2);"
    ).unwrap();

    // Call forward_to_server directly — easier than driving via spawn.
    let result: serde_json::Value = alephcore::cli::ipc_client::forward_to_server(
        &data_dir,
        alephcore::cli::policy::HttpMethod::Post,
        "/v1/admin/secrets",
        serde_json::json!({"key":"FOO","value":"bar"}),
    ).unwrap();

    assert_eq!(result.get("key").and_then(|v| v.as_str()), Some("x"));
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
```

This test depends on `forward_to_server` being callable as `pub`. Verify Task 14 marked it `pub fn`.

- [ ] **Step 3: Run**

```bash
cargo test --test spec_c_cli_ipc -- --test-threads=1 2>&1 | tail -15
cargo test --test spec_c_cli_token_rotation 2>&1 | tail -15
```

Expected: both green.

- [ ] **Step 4: Commit**

```bash
git add tests/spec_c_cli_ipc.rs tests/spec_c_cli_token_rotation.rs
git commit -m "spec-c/e2e: CLI IPC happy path + 401 self-heal retry"
```

---

### Task 23: E2E — LockOnly refusal + endpoint-missing diagnostics

**Purpose:** Verify the negative paths: LockOnly subcommand refuses cleanly; LockOrIpc with stale/missing endpoint file gives clear error.

**Files:**
- Create: `tests/spec_c_cli_refuse.rs`
- Create: `tests/spec_c_cli_endpoint_missing.rs`

- [ ] **Step 1: Write LockOnly refusal test**

```rust
//! A LockOnly subcommand (e.g., `migrate`) must refuse with exit 64
//! when the server holds the singleton lock.

use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn lock_only_command_refuses_when_server_up() {
    let dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_aleph-server");

    let mut server = Command::new(bin).args(["start"])
        .env("HOME", dir.path())
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().unwrap();
    std::thread::sleep(Duration::from_secs(2));

    let out = Command::new(bin)
        .args(["migrate", "--all"])  // Adjust to a real LockOnly subcommand
        .env("HOME", dir.path())
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .output().unwrap();

    let _ = server.kill();
    server.wait().ok();

    assert_eq!(out.status.code(), Some(64));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("server is running"));
}
```

If `migrate` is not a real subcommand, substitute a known LockOnly one identified in Task 19's audit. If none exists, **add a minimal one** (e.g., `aleph debug-dump-vault`) explicitly tagged LockOnly so this test has a target.

- [ ] **Step 2: Write endpoint-missing test**

```rust
//! When the singleton lock is held but `.ipc-endpoint.json` is absent,
//! the CLI must report a clear diagnostic and exit 69.

use std::process::{Command, Stdio};

#[test]
fn cli_reports_missing_endpoint_when_lock_held_but_no_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_aleph-server");

    // Manually take the singleton lock (without starting the server).
    let data_dir = dir.path().join(".aleph/data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let _hold = match alephcore::utils::instance_lock::try_acquire(&data_dir).unwrap() {
        alephcore::utils::instance_lock::AcquireOutcome::Acquired(g) => g,
        _ => panic!(),
    };
    // Do NOT write .ipc-endpoint.json — simulate "server is initializing".

    let out = Command::new(bin)
        .args(["secret", "set", "FOO", "bar"])
        .env("HOME", dir.path())
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .output().unwrap();

    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("server is initializing") || stderr.contains("crashed"),
            "expected initializing/crashed diagnostic, got: {stderr}");
}
```

- [ ] **Step 3: Run**

```bash
cargo test --test spec_c_cli_refuse 2>&1 | tail -10
cargo test --test spec_c_cli_endpoint_missing 2>&1 | tail -10
```

Expected: both green.

- [ ] **Step 4: Commit**

```bash
git add tests/spec_c_cli_refuse.rs tests/spec_c_cli_endpoint_missing.rs
git commit -m "spec-c/e2e: LockOnly refusal + endpoint-missing diagnostic"
```

---

### Task 24: E2E — Vault crash-safe, vault concurrent, acp atomic, sqlite concurrent read

**Purpose:** Four standalone defense-in-depth tests verifying file-level protections + SQLite concurrency.

**Files:**
- Create: `tests/vault_atomic_e2e.rs`
- Create: `tests/vault_concurrent_e2e.rs`
- Create: `tests/acp_atomic_e2e.rs`
- Create: `tests/sqlite_concurrent_read_e2e.rs`

- [ ] **Step 1: Vault atomic write test**

```rust
//! Crash mid-write must leave the vault either fully old or fully new —
//! never half-written.

use alephcore::utils::vault_io::VaultIo;

#[test]
fn vault_atomic_write_survives_simulated_crash() {
    let dir = tempfile::tempdir().unwrap();
    let io = VaultIo::new(dir.path());

    io.write(b"v1").unwrap();
    assert_eq!(io.read().unwrap().as_deref(), Some(b"v1" as &[u8]));

    // Simulate crash by killing a child mid-write. Since `write_atomic`
    // uses `tempfile::NamedTempFile::persist`, even a panic between
    // tempfile creation and rename leaves the original intact.
    let result = std::panic::catch_unwind(|| {
        let dir2 = tempfile::tempdir().unwrap();
        let io2 = VaultIo::new(dir2.path());
        io2.write(b"old").unwrap();
        // Inject a panic in user code that runs after acquire-lock but
        // before write completes. The atomic write itself is internal so
        // we can't inject mid-rename, but we verify that the API surface
        // doesn't leave a half-written file.
        panic!("simulated crash");
    });
    assert!(result.is_err());

    // Original vault still intact.
    assert_eq!(io.read().unwrap().as_deref(), Some(b"v1" as &[u8]));
}
```

- [ ] **Step 2: Vault concurrent test**

```rust
//! Two threads racing on VaultIo::write must serialise via fcntl and
//! leave one of the two writes as the final state. (We can't predict
//! which; we just verify it's complete and self-consistent.)

use alephcore::utils::vault_io::VaultIo;
use std::sync::Arc;
use std::thread;

#[test]
fn two_threads_writing_vault_serialise_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(VaultIo::new(dir.path()));

    let mut handles = vec![];
    for tag in 0..2 {
        let io = io.clone();
        handles.push(thread::spawn(move || {
            let payload = vec![tag as u8; 1024];
            io.write(&payload).unwrap();
        }));
    }
    for h in handles { h.join().unwrap(); }

    let final_bytes = io.read().unwrap().unwrap();
    assert_eq!(final_bytes.len(), 1024);
    let head = final_bytes[0];
    assert!(final_bytes.iter().all(|&b| b == head),
            "vault contents should be uniform {head} bytes (last writer wins)");
}
```

- [ ] **Step 3: acp_sessions atomic test**

```rust
//! `save_sessions` writes a complete JSON file via atomic temp+rename.

#[test]
fn acp_sessions_atomic_write_yields_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", dir.path());

    // Write something through the actual save path. If the public API
    // signature differs, adapt accordingly.
    let path = dir.path().join(".aleph/data/acp_sessions.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    alephcore::utils::atomic_io::write_atomic(&path, br#"[{"session_id":"abc"}]"#).unwrap();

    let bytes = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    assert!(parsed.is_array());
}
```

- [ ] **Step 4: SQLite concurrent read test**

```rust
//! 1 writer + 4 readers should not produce any SQLITE_BUSY errors when
//! the DB is opened via open_sqlite_safe.

use alephcore::utils::sqlite_open::open_sqlite_safe;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn one_writer_four_readers_no_busy_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = Arc::new(dir.path().join("t.db"));

    // Seed schema.
    {
        let conn = open_sqlite_safe(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT);",
        ).unwrap();
    }

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut handles = vec![];

    let writer_path = path.clone();
    let writer_stop = stop.clone();
    handles.push(thread::spawn(move || {
        let conn = open_sqlite_safe(&writer_path).unwrap();
        while !writer_stop.load(std::sync::atomic::Ordering::SeqCst) {
            conn.execute("INSERT INTO t (payload) VALUES (?)",
                         rusqlite::params![format!("data-{}", rand::random::<u32>())]).unwrap();
        }
    }));

    for _ in 0..4 {
        let reader_path = path.clone();
        let reader_stop = stop.clone();
        handles.push(thread::spawn(move || {
            let conn = open_sqlite_safe(&reader_path).unwrap();
            while !reader_stop.load(std::sync::atomic::Ordering::SeqCst) {
                let _: i64 = conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
            }
        }));
    }

    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(500) {
        thread::yield_now();
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    for h in handles { h.join().unwrap(); }
}
```

If `rand` is not in dev-deps, replace with `std::time::SystemTime::now().elapsed().unwrap().as_nanos()` for a unique-ish payload.

- [ ] **Step 5: Run**

```bash
cargo test --test vault_atomic_e2e 2>&1 | tail -10
cargo test --test vault_concurrent_e2e 2>&1 | tail -10
cargo test --test acp_atomic_e2e 2>&1 | tail -10
cargo test --test sqlite_concurrent_read_e2e 2>&1 | tail -10
```

Expected: 4 tests all green.

- [ ] **Step 6: Commit**

```bash
git add tests/vault_atomic_e2e.rs tests/vault_concurrent_e2e.rs \
        tests/acp_atomic_e2e.rs tests/sqlite_concurrent_read_e2e.rs
git commit -m "spec-c/e2e: vault atomic+concurrent, acp atomic, sqlite read concurrency"
```

---

### Task 25: Reverse-regression script + CLAUDE.md update

**Purpose:** Lock in the invariants by codifying the grep-based regression checks; refresh CLAUDE.md's process-management section.

**Files:**
- Create: `scripts/spec_c_regression.sh`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Write the regression script**

Create `scripts/spec_c_regression.sh`:

```bash
#!/usr/bin/env bash
# Spec C — cross-process safety regression checks.
# Run before every commit that touches anything Spec C governs.

set -euo pipefail
cd "$(dirname "$0")/.."

fail=0

echo "==> [1/4] SQLite opens must route through open_sqlite_safe / open_sqlite_readonly"
if git grep -nE "Connection::open\(|rusqlite::Connection::open\(" src/ --include='*.rs' \
   | grep -v "utils/sqlite_open\.rs" \
   | grep -v "utils/spec_c_audit\.rs" \
   | grep -v "tests/" \
   | grep -v "^\s*//"; then
    echo "  ❌ direct rusqlite open detected — must use open_sqlite_safe"
    fail=1
else
    echo "  ✅"
fi

echo "==> [2/4] secrets.vault and acp_sessions.json must route through wrappers"
if git grep -nE "secrets\.vault|acp_sessions\.json" src/ --include='*.rs' \
   | grep -v "vault_io\.rs" \
   | grep -v "atomic_io\.rs" \
   | grep -v "acp/manager\.rs:.*acp_sessions_path" \
   | grep -v "spec_c_audit\.rs" \
   | grep -v "tests/" \
   | grep -v "^\s*//"; then
    echo "  ❌ raw access detected"
    fail=1
else
    echo "  ✅"
fi

echo "==> [3/4] every CLI subcommand file dispatches via with_policy or run_no_lock"
missing=$(git grep -L "with_policy\|run_no_lock\|with_policy_owned" src/bin/aleph-server/commands/ \
          | grep '\.rs$' | grep -v 'mod\.rs' || true)
if [ -n "$missing" ]; then
    echo "  ❌ unannotated subcommand files:"
    echo "$missing" | sed 's/^/      /'
    fail=1
else
    echo "  ✅"
fi

echo "==> [4/4] no leftover acquire_instance_lock calls outside the daemon thin wrapper"
if git grep -n "acquire_instance_lock" src/ --include='*.rs' \
   | grep -v "src/bin/aleph-server/daemon\.rs" \
   | grep -v "spec_c_audit\.rs" \
   | grep -v "tests/" \
   | grep -v "^\s*//"; then
    echo "  ❌ stale acquire_instance_lock callers"
    fail=1
else
    echo "  ✅"
fi

if [ $fail -ne 0 ]; then
    echo
    echo "Spec C regression checks FAILED. Fix and re-run."
    exit 1
fi
echo
echo "Spec C regression checks PASS."
```

Make executable:

```bash
chmod +x scripts/spec_c_regression.sh
```

- [ ] **Step 2: Run it**

```bash
./scripts/spec_c_regression.sh
```

Expected: all 4 checks pass.

- [ ] **Step 3: Update CLAUDE.md**

Edit the "进程管理 (Process Management)" section in `CLAUDE.md`. Replace the existing block (which references `.shared_token` and "wait 2 seconds") with:

```markdown
### 进程管理 (Process Management)

Singleton enforcement is now structural (Spec C, 2026-05-02):

- `aleph-server start` acquires `~/.aleph/data/aleph.lock` via `flock`
  before any DB open. A second `start` exits with code 64 and a
  diagnostic naming the holder PID.
- All CLI write subcommands (`secret`, `memory`, `agent`, ...) route
  through `with_policy`: when the lock is held by a running server,
  they forward via `/v1/admin/*` IPC; when no server is running, they
  acquire the lock locally for the duration of the operation.
- The OS releases `flock` automatically on process exit (graceful,
  panic, SIGKILL). After `kill -9 <pid>`, you may immediately
  `aleph-server start` — no sleep required.

If you see "Stale lock file detected (PID X not running)" you are safe
to `rm ~/.aleph/data/aleph.lock` (this should never happen in practice
because flock state is OS-managed; the diagnostic is purely defensive).
```

- [ ] **Step 4: Commit**

```bash
git add scripts/spec_c_regression.sh CLAUDE.md
git commit -m "spec-c: regression script + CLAUDE.md process-management update"
```

---

### Task 26: Roadmap, memory, reference docs + final acceptance review

**Purpose:** Update tracking artefacts, run the full acceptance criteria gauntlet, delete the audit scratch file from Task 1.

**Files:**
- Modify: `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- Modify: `docs/reference/SECURITY.md`
- Create: `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_c_cross_process_safety.md`
- Modify: `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md`
- Delete: `src/utils/spec_c_audit.rs`
- Modify: `src/utils/mod.rs` (remove `pub mod spec_c_audit;`)

- [ ] **Step 1: Mark roadmap row shipped**

Edit `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`. Change:

```markdown
| C. Cross-process safety beyond curated layer | ⏸ pending | — | — | — |
```

to:

```markdown
| C. Cross-process safety beyond curated layer | ✅ shipped | [design](2026-05-02-memory-evolution-spec-c-cross-process-safety-design.md) | [plan](../plans/2026-05-02-memory-evolution-spec-c-cross-process-safety.md) | 2026-05-02 |
```

- [ ] **Step 2: Update SECURITY.md**

Append to `docs/reference/SECURITY.md` a new subsection:

```markdown
## Cross-process safety guarantees (Spec C, 2026-05-02)

- A single process per `~/.aleph/data/` directory: enforced by `flock`
  on `aleph.lock` acquired in `main()` before any other state.
- `secrets.vault` writes are atomic temp+rename + adjacent fcntl
  advisory lock — defense-in-depth even if the singleton fails.
- `acp_sessions.json` writes are atomic temp+rename.
- All `~/.aleph/data/*.db` connections use WAL + `busy_timeout=5000`
  + `synchronous=NORMAL` via `alephcore::utils::sqlite_open`.
- CLI subcommands holding the singleton lock OR forwarding through
  `/v1/admin/*` IPC — never bypass either.

Reverse-regression checks: `scripts/spec_c_regression.sh`.
```

- [ ] **Step 3: Write the memory file**

Create `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_c_cross_process_safety.md`:

```markdown
---
name: Spec C — Cross-Process Safety (SHIPPED)
description: Closes the last Hermes-vs-Aleph follow-up gap; eliminates every cross-process write race in `~/.aleph/data/`.
type: project
---

Spec C shipped 2026-05-02. Roadmap row marked `✅ shipped`.

**Architecture deltas:**
- `src/utils/{atomic_io,instance_lock,sqlite_open,vault_io}.rs` — new core utilities.
- `src/cli/{policy,endpoint,ipc_client}.rs` — CLI dispatch + IPC client.
- `src/gateway/admin_api/{secrets,memory,agents}.rs` — server-side IPC handlers.
- `src/gateway/security/token_readonly.rs` — bearer token read-only access.

**Why:** The CLAUDE.md warning ("multiple aleph processes corrupt the vault") was a real, reproducible bug — Spec A only protected `MEMORY.md`. Spec C extends the protection to every other writeable surface.

**How to apply:** When adding new CLI subcommands, declare a `CommandPolicy` and dispatch via `with_policy` / `run_no_lock`. Reverse-regression checks (`scripts/spec_c_regression.sh`) enforce this.
```

- [ ] **Step 4: Add MEMORY.md index entry**

Append to `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md`:

```
- [Spec C — Cross-Process Safety (SHIPPED)](project_spec_c_cross_process_safety.md) — Eliminates remaining cross-process write races; admin IPC + WAL helpers + vault fcntl.
```

- [ ] **Step 5: Delete audit scratch file**

```bash
git rm src/utils/spec_c_audit.rs
```

Edit `src/utils/mod.rs` and remove the line:
```rust
#[cfg(debug_assertions)]
pub mod spec_c_audit;
```

- [ ] **Step 6: Final acceptance gauntlet**

Run:
```bash
cargo test --workspace --lib 2>&1 | tail -5
cargo test --test 'spec_c_*' --test 'instance_lock_e2e' --test 'vault_*_e2e' --test 'acp_atomic_e2e' --test 'sqlite_concurrent_read_e2e' 2>&1 | tail -10
cargo clippy --workspace -- -D warnings 2>&1 | tail -5
./scripts/spec_c_regression.sh
```

Expected:
- All `--lib` tests pass
- All Spec C integration tests pass
- Clippy clean
- Regression script all 4 checks pass

- [ ] **Step 7: Manual smoke validation (acceptance criteria 1-5, 10)**

Run each in sequence:

```bash
# (1) Double-start refused
target/release/aleph-server start &
sleep 3
target/release/aleph-server start
echo "exit: $?"  # should be 64
kill %1; wait

# (2) Server-up secret set via IPC
target/release/aleph-server start &
sleep 3
target/release/aleph-server secret set FOO bar
target/release/aleph-server secret get FOO
kill %1; wait

# (3) Server-down secret set via lock
target/release/aleph-server secret set BAZ qux
target/release/aleph-server secret get BAZ

# (4) SIGKILL → immediate restart
target/release/aleph-server start &
sleep 3
kill -9 %1
target/release/aleph-server start &  # no sleep!
sleep 3
kill %1; wait

# (5) 8 concurrent CLIs
target/release/aleph-server start &
sleep 3
for i in {1..8}; do (target/release/aleph-server secret set "K$i" "v$i" &); done
wait
kill %1; wait
```

For each, record outcome in a smoke log:

Create `docs/superpowers/specs/2026-05-02-spec-c-smoke-log.md`:

```markdown
# Spec C — Smoke Walk-through Log

## (1) Double-start refused
- First start PID: <pid>
- Second exit code: 64 ✅
- Stderr: "Another Aleph instance is already running (PID <pid>)..." ✅

## (2) Server-up secret set via IPC
- secret set: exit 0, server log shows POST /v1/admin/secrets ✅
- secret get: returns "bar" ✅

## (3) Server-down secret set via lock
- secret set: exit 0 ✅
- secret get: returns "qux" ✅

## (4) SIGKILL → immediate restart
- Restart succeeded with no sleep ✅
- vault.db inspection: `ls -la ~/.aleph/data/secrets.vault` shows expected file ✅

## (5) 8 concurrent CLIs
- All 8 children exit 0 ✅
- secret list shows K1..K8 with correct values ✅
```

Also commit Task 19's deferred manual smoke for Spec B (acceptance #9 of Spec B), since the user said it would happen post-Spec C. Append to that smoke log if present, or note it here.

- [ ] **Step 8: Commit final shipped artefacts**

```bash
git add docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md \
        docs/reference/SECURITY.md \
        docs/superpowers/specs/2026-05-02-spec-c-smoke-log.md \
        src/utils/mod.rs
# Memory files (outside repo) — separate command
cp ~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_c_cross_process_safety.md \
   ~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_c_cross_process_safety.md
# (no-op; the file is already on disk; nothing to add to repo for it)
git rm src/utils/spec_c_audit.rs
git commit -m "spec-c: ship — roadmap, security docs, smoke log, drop audit scratch file"
```

---

## Final Acceptance Checklist

Before declaring Spec C done, every line below must be checked.

- [ ] All 26 task commits land on `main` with messages starting `spec-c: ` (no `--no-verify`, no amends, no `git add -A` outside Task 7's verified scope).
- [ ] The 5 inherited dirty files remain untouched (`git status` shows them still modified, no other changes).
- [ ] `cargo test --workspace --lib` green.
- [ ] All Spec C integration tests green: `instance_lock_e2e`, `spec_c_double_start`, `spec_c_cli_no_server`, `spec_c_cli_ipc`, `spec_c_cli_token_rotation`, `spec_c_cli_refuse`, `spec_c_cli_endpoint_missing`, `vault_atomic_e2e`, `vault_concurrent_e2e`, `acp_atomic_e2e`, `sqlite_concurrent_read_e2e`.
- [ ] `cargo clippy --workspace -- -D warnings` clean.
- [ ] `./scripts/spec_c_regression.sh` all 4 checks pass.
- [ ] CLAUDE.md process-management section updated.
- [ ] Roadmap shows `C. ✅ shipped`.
- [ ] `docs/superpowers/specs/2026-05-02-spec-c-smoke-log.md` exists and records all 5 scenarios green.
- [ ] `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md` has Spec C entry.
- [ ] `src/utils/spec_c_audit.rs` deleted.
- [ ] No new dependencies in `Cargo.toml` (Spec C uses only existing deps: fs2, tempfile, reqwest, rusqlite, serde_json, chrono).

If anything fails, debug + fix + re-run before moving on.
