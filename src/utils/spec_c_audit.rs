//! # Spec C — Cross-Process Safety Audit Notes
//!
//! Temporary audit-only file. **Deleted in Task 26** (final acceptance).
//!
//! Captured 2026-05-02 by Task 1 of the Spec C plan
//! (`docs/superpowers/plans/2026-05-02-memory-evolution-spec-c-cross-process-safety.md`).
//!
//! ## Singleton (current state, Task 3 will migrate)
//! - Path: `~/.aleph/data/aleph.lock`
//! - Implementation: `src/bin/aleph-server/daemon.rs:62-127` (Unix `#[cfg(unix)]` arm)
//!   plus a `#[cfg(not(unix))]` stub at `src/bin/aleph-server/daemon.rs:129+` (also at line 133
//!   joins `.aleph/data/aleph.lock`).
//! - Flock: `unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) }` on a file opened with
//!   `OpenOptions::new().create(true).write(true).truncate(false)`. Lock file body is the PID
//!   (newline-terminated) for diagnostics; OS releases the flock on process exit so no stale
//!   lock cleanup is needed.
//! - Stale-PID branch in error message uses `is_process_running(pid)` (same daemon.rs).
//! - Call sites (production):
//!   - `src/bin/aleph-server/commands/start/mod.rs:374`
//!     `let _instance_lock = crate::daemon::acquire_instance_lock()?;`
//!   - No other production callers — `start` is the sole owner. Definition is at daemon.rs:62.
//!
//! ## SQLite `Connection::open` sites (24 total in production code, no `_test` excluded)
//!
//! Already WAL + busy_timeout=5000 (7 sites; Task 7 will migrate to helper):
//! - `src/tasks/heartbeat/store.rs:40` (open) → `:44` (`PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;`)
//! - `src/tasks/shared/store.rs:25`     (open) → `:28` (same PRAGMA)
//! - `src/tasks/cron/store.rs:46`       (open) → `:50` (same PRAGMA)
//! - `src/bin/aleph-server/commands/start/builder/agent_init.rs:184` → `:188` (PRAGMA)
//! - `src/bin/aleph-server/commands/start/builder/agent_init.rs:259` → `:262` (PRAGMA, macro arm)
//! - `src/bin/aleph-server/commands/start/builder/agent_init.rs:303` → `:306` (PRAGMA)
//! - `src/memory/store/sqlite/mod.rs:64` → `:72` (`PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;`
//!   — note: sets WAL but NOT busy_timeout; helper migration must preserve `synchronous=NORMAL`)
//!
//! Missing WAL+busy_timeout (17 sites; Task 7 will migrate):
//! - `src/bin/aleph-server/commands/audit.rs:262`
//! - `src/bin/aleph-server/commands/start/mod.rs:242` (`rusqlite::Connection::open`)
//! - `src/bin/aleph-server/commands/start/mod.rs:1436` (`rusqlite::Connection::open`)
//! - `src/bin/aleph-server/commands/start/builder/agent_init.rs:127` (`rusqlite::Connection::open`,
//!   no PRAGMA at this site — only the 184/259/303 arms add it)
//! - `src/components/session_recorder/mod.rs:55`
//! - `src/resilience/database/state_database/mod.rs:101`
//! - `src/resilience/database/state_database/mod.rs:177`
//! - `src/exec/approval/storage.rs:11`
//! - `src/engine/persistence.rs:43`
//! - `src/gateway/pairing_store.rs:90`
//! - `src/gateway/session_manager/mod.rs:254`
//! - `src/gateway/agent_env/mod.rs:482`
//! - `src/gateway/session_store/migration.rs:44`
//! - `src/gateway/device_store.rs:50`
//! - `src/gateway/interfaces/msteams/polls.rs:66`
//! - `src/gateway/security/store.rs:23`
//! - `src/gateway/interfaces/matrix/dedupe.rs:15`
//!
//! ## `secrets.vault` access
//! - Canonical path builder: `src/secrets/vault.rs:182-185` (uses `dirs::home_dir().join(".aleph/secrets.vault")`,
//!   falls back to relative `secrets.vault` if `home_dir()` fails). **Note**: the path builder uses
//!   `~/.aleph/secrets.vault` (NOT `~/.aleph/data/secrets.vault`); two CLI sites instead build
//!   `data_dir.join("secrets.vault")` — Task 8 must reconcile this divergence.
//! - Secondary path-build sites (use `data_dir.join("secrets.vault")` — i.e. `~/.aleph/data/secrets.vault`):
//!   - `src/bin/aleph-server/commands/secret.rs:57`
//!   - `src/bin/aleph-server/commands/start/builder/subsystems.rs:98`
//! - Direct `std::fs::*` sites on the vault file:
//!   - `src/secrets/vault.rs:35`  `let bytes = std::fs::read(&path)?;` (load path)
//!   - `src/secrets/vault.rs:73`  `if let Err(e) = std::fs::write(&tmp_path, &bytes) { ... }`
//!     (already writes to `<path>.tmp` — partial atomicity; Task 8 must add fsync + atomic rename)
//!
//! ## `acp_sessions.json` access
//! - Canonical path builder: `src/acp/manager.rs:24` `fn acp_sessions_path()` →
//!   `~/.aleph/data/acp_sessions.json` (assert at `:962` confirms suffix).
//! - Direct `std::fs::*` sites:
//!   - `src/acp/manager.rs:35`  `match std::fs::read_to_string(&path) { ... }` (load path)
//!   - `src/acp/manager.rs:53`  `if std::fs::write(&tmp, &json).is_ok() { ... }`
//!     (writes to `<path>.tmp` then renames — already partial atomicity; Task 9 will switch to the
//!     `write_atomic` helper from Task 2 and add fsync.)
//!
//! ## SharedTokenManager active-token query
//! - In-memory accessor (no SQL): `src/gateway/security/shared_token.rs:153`
//!   `pub fn get_current_token(&self) -> Option<String>` — clones from `self.current_token: Mutex<Option<String>>`.
//!   This is a process-local cache and is NOT what Task 13 needs to mirror.
//! - DB-level read: `src/gateway/security/store.rs:566-572` in
//!   `pub fn get_shared_token_plaintext(&self) -> SqliteResult<Option<String>>`. Verbatim SQL:
//!   ```text
//!   SELECT plaintext_token FROM shared_token WHERE plaintext_token IS NOT NULL LIMIT 1
//!   ```
//!   Connection method: `conn.query_row(SQL, [], |row| row.get::<_, String>(0))`. Empty-string and
//!   `QueryReturnedNoRows` both map to `Ok(None)`. **Task 13's `read_current_token_readonly` must
//!   mirror this SELECT byte-for-byte.**
//!
//! ## CLI subcommands (`src/bin/aleph-server/commands/`)
//! All files declare `pub fn` or `pub async fn` entry points (verified via `grep -l`):
//! - `start` (directory module): write+read+long-running → tentative policy **NoLock at top-level
//!   dispatch, but acquires the singleton internally** (start IS the lock holder; treat as `NoLock`
//!   from `with_policy`'s perspective)
//! - `gateway.rs`: read-mostly status/admin → tentative **LockOrIpc**
//! - `audit.rs`: opens audit DB read-only → tentative **NoLock** (read-only SQLite)
//! - `devices.rs`: device list/register, touches gateway DB → tentative **LockOrIpc**
//! - `secret.rs`: vault CRUD via `data_dir.join("secrets.vault")` → tentative **LockOrIpc** (must
//!   route through server when running, otherwise direct write under flock)
//! - `pairing.rs`: pairing flow, touches `pairing_store` (SQLite) → tentative **LockOrIpc**
//! - `plugins.rs`: plugin install/list → tentative **LockOrIpc** (writes vault for plugin secrets,
//!   reads SQLite plugin registry)
//! - `bootstrap_runtime` (directory module): runtime bootstrap helpers, internal use by `start` →
//!   not a user-facing subcommand; tentative **NoLock**
//! - `mod.rs`: dispatcher only — no policy needed
//!
//! Net user-facing subcommands: 7 (`start`, `gateway`, `audit`, `devices`, `secret`, `pairing`,
//! `plugins`). Plan estimate (8-15) was pessimistic; actual count is 7. Task 19 will finalize each
//! policy after Task 10's `CommandPolicy` enum lands.
//!
//! ## reqwest in deps
//! - **Yes.** `Cargo.toml` declares `reqwest = { version = "0.12", features = ["json", "native-tls",
//!   "stream", "multipart"] }` (also a stale `version = "0.11"` line appears in the same file —
//!   workspace vs crate; Task 14 should pick the 0.12 line). Plus `reqwest-eventsource = "0.6"`.
//! - Existing `reqwest::Client` users: `src/memory/embedding_provider.rs:53,68`,
//!   `src/memory/rerank/{siliconflow,jina,vllm}.rs`. No new dependency needed for Task 14's CLI
//!   HTTP forwarder.
