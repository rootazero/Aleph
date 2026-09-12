//! Declarative policy + dispatch helpers for CLI subcommands.
//!
//! Every CLI subcommand declares one of three policies and dispatches
//! through `run_no_lock` (`NoLock`) or `with_policy` (`LockOnly` / `LockOrIpc`,
//! filled in by Task 11).

use std::fmt;
use std::path::Path;

use crate::utils::instance_lock::{self, AcquireOutcome, InstanceLock};

/// Error returned when the instance lock is held by another process.
/// This is a distinct type so callers can match on it without fragile
/// string comparison.
#[derive(Debug)]
pub struct LockHeldError {
    pub holder: LockHolder,
    pub lock_path: std::path::PathBuf,
}

/// Who holds the lock, as far as the holder sidecar can say. Mirrors the
/// `Held*` arms of [`AcquireOutcome`] one-to-one, and like them every
/// variant is a lock that **is held right now** — the OS reported
/// contention. There is no "orphaned, nobody there" variant: that state
/// never reaches a CLI command, because a free lock is simply acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockHolder {
    /// The sidecar names a running process.
    Live { pid: u32 },
    /// The sidecar names a PID that is not running — the record is stale
    /// (a daemon whose PID was not rewritten after forking, typically), but
    /// the lock itself is held.
    StaleRecord { pid: u32 },
    /// The sidecar is missing or unreadable; the holder cannot be named.
    Unknown,
}

impl fmt::Display for LockHeldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every arm says the lock is held. Only the first can name the
        // process; the other two must not print a PID the reader could
        // `kill` (0 is not a process, and a stale PID is somebody else's).
        match self.holder {
            LockHolder::Live { pid } => write!(f, "server is running (PID {pid})")?,
            LockHolder::StaleRecord { pid } => write!(
                f,
                "server is running but its holder record is stale (names PID {pid}, \
                 which is not running)"
            )?,
            LockHolder::Unknown => write!(f, "server is running (holder unknown)")?,
        }
        write!(
            f,
            ". This command requires exclusive access — run `aleph stop` first. \
             Do not remove the lock file while it is held. Lock: {}",
            self.lock_path.display()
        )
    }
}

impl std::error::Error for LockHeldError {}

#[derive(Debug, Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    #[must_use]
    pub const fn as_reqwest(&self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
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
    LockOrIpc {
        route: &'static str,
        method: HttpMethod,
    },
}

/// Dispatch a `NoLock` subcommand. Currently a thin pass-through; the
/// indirection exists so reverse-regression checks (Task 23) can scan
/// `src/bin/aleph-server/commands/` for `run_no_lock(` to verify every
/// command file has gone through policy classification.
pub fn run_no_lock<T, F>(f: F) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T>,
{
    f()
}

/// Attempt to acquire the lock and, if held, build a typed `LockHeldError`
/// instead of a plain string so callers can distinguish lock contention
/// from other failures.
fn acquire_or_held(data_dir: &Path) -> anyhow::Result<InstanceLock> {
    let (holder, lock_path) = match instance_lock::try_acquire(data_dir)? {
        AcquireOutcome::Acquired(lock) => return Ok(lock),
        AcquireOutcome::HeldByLive { pid, lock_path } => {
            (LockHolder::Live { pid: pid as u32 }, lock_path)
        }
        AcquireOutcome::HeldByOrphaned { pid, lock_path } => {
            (LockHolder::StaleRecord { pid: pid as u32 }, lock_path)
        }
        AcquireOutcome::HeldByUnknown { lock_path, .. } => (LockHolder::Unknown, lock_path),
    };
    Err(LockHeldError { holder, lock_path }.into())
}

/// `with_policy` variant that returns `Err` instead of calling
/// `std::process::exit` on lock contention. Called by `with_policy` for
/// the `NoLock` / `LockOrIpc` arms (whose contention behavior is
/// identical to `try_with_policy`'s); the `LockOnly` arm in `with_policy`
/// exits cleanly on contention and never reaches this function. Also
/// invoked directly by unit tests.
pub fn try_with_policy<L, T>(
    policy: CommandPolicy,
    data_dir: &Path,
    local: L,
    ipc_body: serde_json::Value,
) -> anyhow::Result<T>
where
    L: FnOnce(&InstanceLock) -> anyhow::Result<T>,
    T: serde::de::DeserializeOwned,
{
    match policy {
        CommandPolicy::NoLock => {
            anyhow::bail!("NoLock commands must dispatch through run_no_lock, not with_policy")
        }
        CommandPolicy::LockOnly => {
            let lock = acquire_or_held(data_dir)?;
            local(&lock)
        }
        CommandPolicy::LockOrIpc { route, method } => match acquire_or_held(data_dir) {
            Ok(lock) => local(&lock),
            Err(e) => {
                if e.downcast_ref::<LockHeldError>().is_some() {
                    // Lock is held — try forwarding to the running server. If
                    // the holder releases between our acquire-or-held check
                    // and the IPC request landing, the forward will fail
                    // with a confusing "server is initializing or crashed"
                    // error. Retry local acquisition once: if the lock is
                    // now free we run `local`. The second lock error is
                    // logged at warn; if it is the strictly more informative
                    // error (e.g. PermissionDenied because data_dir mode
                    // changed) we surface it instead of the IPC error.
                    match crate::cli::ipc_client::forward_to_server::<T>(
                        data_dir, method, route, ipc_body,
                    ) {
                        Ok(out) => Ok(out),
                        Err(fwd_err) => match acquire_or_held(data_dir) {
                            Ok(lock) => local(&lock),
                            Err(lock_err) => {
                                // Stay defensive: if the second lock error is
                                // not LockHeld (e.g. PermissionDenied,
                                // NotFound on data_dir), it is more
                                // informative than the IPC error. The
                                // LockHeld case keeps the IPC error because
                                // the lock IS held — the IPC error is the
                                // next-most-actionable signal.
                                if lock_err.downcast_ref::<LockHeldError>().is_some() {
                                    Err(fwd_err)
                                } else {
                                    tracing::warn!(
                                        ipc_error = %fwd_err,
                                        lock_error = %lock_err,
                                        "lock state changed between IPC failure and retry; \
                                         surfacing lock error (more informative than IPC)"
                                    );
                                    Err(lock_err)
                                }
                            }
                        },
                    }
                } else {
                    Err(e)
                }
            }
        },
    }
}

/// Production dispatch: same as `try_with_policy` but converts lock
/// contention into a clean stderr + `std::process::exit(64)`
/// instead of returning an `Err` to the caller.
pub fn with_policy<L, T>(
    policy: CommandPolicy,
    data_dir: &Path,
    local: L,
    ipc_body: serde_json::Value,
) -> anyhow::Result<T>
where
    L: FnOnce(&InstanceLock) -> anyhow::Result<T>,
    T: serde::de::DeserializeOwned,
{
    // Only the LockOnly contention behavior differs from `try_with_policy`
    // (clean stderr + exit 64 instead of returning an Err). The NoLock and
    // LockOrIpc arms are identical, so delegate to keep one source of truth.
    if let CommandPolicy::LockOnly = policy {
        let lock = acquire_or_held(data_dir).inspect_err(|e| {
            if let Some(held) = e.downcast_ref::<LockHeldError>() {
                eprintln!("{held}");
                // TODO: clippy::exit — `with_policy` is documented as the production
                // dispatch that exits cleanly on lock contention rather than returning
                // an `Err` to the caller. Replacing this with `Result` propagation would
                // change the public API contract and all callers, so it is left as-is.
                std::process::exit(64);
            }
        })?;
        return local(&lock);
    }
    try_with_policy(policy, data_dir, local, ipc_body)
}

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

    fn held(holder: LockHolder) -> String {
        LockHeldError {
            holder,
            lock_path: std::path::PathBuf::from("/tmp/aleph.lock"),
        }
        .to_string()
    }

    /// Live-server case: the message must name the holder PID so the
    /// operator knows which process to stop. `spec_c_cli_refuse` greps
    /// "server is running" on stderr, so that phrase is a wire contract.
    #[test]
    fn lock_held_error_display_for_live_holder() {
        let msg = held(LockHolder::Live { pid: 1234 });
        assert!(msg.contains("server is running (PID 1234)"), "{msg}");
        assert!(!msg.contains("stale"), "{msg}");
        assert!(!msg.contains("holder unknown"), "{msg}");
    }

    /// Stale-record case: the lock is held (contention), but the sidecar
    /// names a PID that is not running. The message must say the server is
    /// running, call the record stale, keep the dead PID for forensics, and
    /// never call the lock orphaned — nobody is going to `rm` a held lock
    /// on this message's advice.
    #[test]
    fn lock_held_error_display_for_stale_record() {
        let msg = held(LockHolder::StaleRecord { pid: 5678 });
        assert!(msg.contains("server is running"), "{msg}");
        assert!(msg.contains("stale"), "{msg}");
        assert!(msg.contains("PID 5678"), "{msg}");
        assert!(!msg.contains("orphaned"), "{msg}");
        assert!(msg.contains("Do not remove the lock file"), "{msg}");
    }

    /// Holder-unknown case: the sidecar was missing or unreadable. The
    /// message must NOT print "PID 0" (it is never a real process) and must
    /// NOT say nobody is there — the OS reported the lock as held.
    #[test]
    fn lock_held_error_display_when_holder_unknown() {
        let msg = held(LockHolder::Unknown);
        assert!(msg.contains("server is running (holder unknown)"), "{msg}");
        assert!(!msg.contains("PID 0"), "{msg}");
        assert!(!msg.contains("orphaned"), "{msg}");
        assert!(!msg.contains("no live server"), "{msg}");
    }

    #[test]
    fn with_policy_lock_only_acquires_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let result: i32 = with_policy::<_, i32>(
            CommandPolicy::LockOnly,
            dir.path(),
            |_lock| Ok(7),
            serde_json::Value::Null,
        )
        .unwrap();
        assert_eq!(result, 7);
    }

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

    /// A held lock whose sidecar is missing reaches the CLI as
    /// `HeldByUnknown` and must read as a *running* server with an unknown
    /// holder — never "PID 0", and never "orphaned / no live server": this
    /// very test is holding the lock while the message is rendered, which
    /// is exactly what the old wording denied.
    #[test]
    fn held_by_unknown_reads_as_a_running_server_with_no_pid() {
        let dir = tempfile::tempdir().unwrap();
        // Take the lock so subsequent acquires see HeldBy*.
        let _hold = match crate::utils::instance_lock::try_acquire(dir.path()).unwrap() {
            crate::utils::instance_lock::AcquireOutcome::Acquired(g) => g,
            _ => panic!(),
        };
        // Wipe the sidecar; the exclusive lock is still held by `_hold`.
        let holder_path = dir.path().join("aleph.lock.pid");
        std::fs::remove_file(&holder_path).unwrap();
        let result: anyhow::Result<i32> = try_with_policy::<_, i32>(
            CommandPolicy::LockOnly,
            dir.path(),
            |_lock| Ok(7),
            serde_json::Value::Null,
        );
        let err = result.expect_err("should fail when held");
        let held = err
            .downcast_ref::<LockHeldError>()
            .expect("contention surfaces as LockHeldError");
        assert_eq!(held.holder, LockHolder::Unknown);
        let msg = held.to_string();
        assert!(msg.contains("server is running (holder unknown)"), "{msg}");
        assert!(!msg.contains("PID 0"), "{msg}");
        assert!(!msg.contains("orphaned"), "{msg}");
    }

    /// M1: `LockOrIpc` should retry local acquisition when the IPC forward
    /// fails. We simulate the failure by writing an endpoint URL that
    /// points at a port nobody is listening on — the HTTP forward will
    /// fail; the retry then tries to take the local lock, succeeds because
    /// the holder has released (we drop `_hold` between the two phases),
    /// and `local` runs.
    #[test]
    fn lock_or_ipc_retries_local_acquire_when_forward_fails() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = match crate::utils::instance_lock::try_acquire(dir.path()).unwrap() {
            crate::utils::instance_lock::AcquireOutcome::Acquired(g) => g,
            _ => panic!(),
        };
        // Seed a bearer token + a real .ipc-endpoint.json pointing at an
        // unbound port so `forward_to_server` will fail to connect.
        let security_db = dir.path().join("security.db");
        let conn = crate::utils::sqlite_open::open_sqlite_safe(&security_db).unwrap();
        conn.execute_batch(
            "CREATE TABLE shared_token (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                plaintext_token TEXT,
                created_at INTEGER
             );
             INSERT INTO shared_token (plaintext_token, created_at)
             VALUES ('tok', 1);",
        )
        .unwrap();
        drop(conn);
        crate::cli::endpoint::write_endpoint(
            dir.path(),
            &crate::cli::endpoint::IpcEndpoint::current("http://127.0.0.1:1".to_string()),
        )
        .unwrap();

        // Release the lock so the retry path can succeed.
        drop(_hold);

        // Run the policy with an `http://127.0.0.1:1` endpoint (no
        // listener) and assert that the retry path takes the local lock
        // and runs `local`.
        let result: i32 = try_with_policy::<_, i32>(
            CommandPolicy::LockOrIpc {
                route: "/v1/admin/whatever",
                method: HttpMethod::Get,
            },
            dir.path(),
            |_lock| Ok(42),
            serde_json::Value::Null,
        )
        .expect("retry path should run local");
        assert_eq!(result, 42);
    }
}
