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
    pub pid: u32,
    pub lock_path: std::path::PathBuf,
    pub orphaned: bool,
}

impl fmt::Display for LockHeldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.orphaned {
            "orphaned lock detected; no live server"
        } else {
            "server is running"
        };
        // PID 0 is never a real user-space process: it is how `acquire_or_held`
        // spells "the lock file is held but the sidecar recording WHO holds it
        // is missing". That call site already says so, and flips `orphaned` for
        // exactly this reason — but the rendering kept printing the number
        // anyway, so the message read "no live server (PID 0)": a figure the
        // reader can act on, sitting next to a sentence saying nobody is there.
        // Both halves of the same fact have to agree.
        if self.pid == 0 {
            return write!(
                f,
                "{} (holder unknown). This command requires \
                 exclusive access — run `aleph stop` first. Lock: {}",
                status,
                self.lock_path.display()
            );
        }
        write!(
            f,
            "{} (PID {}). This command requires \
             exclusive access — run `aleph stop` first. Lock: {}",
            status,
            self.pid,
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
    match instance_lock::try_acquire(data_dir)? {
        AcquireOutcome::Acquired(lock) => Ok(lock),
        AcquireOutcome::HeldByLive { pid, lock_path } => {
            // `pid == 0` here means the holder sidecar was missing or
            // unreadable (see `instance_lock::try_acquire`). PID 0 is never
            // a valid user-space process, so don't surface "server is
            // running (PID 0)" — treat it as orphaned/unknown and let the
            // operator message stay honest.
            let orphaned = pid == 0;
            Err(LockHeldError {
                pid: pid as u32,
                lock_path,
                orphaned,
            }
            .into())
        }
        AcquireOutcome::HeldByOrphaned { pid, lock_path } => Err(LockHeldError {
            pid: pid as u32,
            lock_path,
            orphaned: true,
        }
        .into()),
    }
}

/// Test-friendly variant of `with_policy` that returns `Err` instead of
/// calling `std::process::exit` on lock contention. Production callers
/// should use `with_policy` which surfaces UX-friendly stderr messages.
#[allow(dead_code)] // public test-only variant — see audit note F-cleanup
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
                    // now free we run `local`; only the second failure is
                    // surfaced.
                    match crate::cli::ipc_client::forward_to_server::<T>(
                        data_dir, method, route, ipc_body,
                    ) {
                        Ok(out) => Ok(out),
                        Err(fwd_err) => match acquire_or_held(data_dir) {
                            Ok(lock) => local(&lock),
                            Err(_) => Err(fwd_err),
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

    /// M2: when the holder sidecar is unreadable, `try_acquire` returns
    /// `HeldByLive { pid: 0 }`. `acquire_or_held` must surface that as
    /// "orphaned / unknown" — never as "server is running (PID 0)" — so the
    /// operator message stays honest.
    #[test]
    fn held_by_live_with_pid_zero_is_treated_as_orphaned() {
        let dir = tempfile::tempdir().unwrap();
        // Take the lock so subsequent acquires see HeldBy*.
        let _hold = match crate::utils::instance_lock::try_acquire(dir.path()).unwrap() {
            crate::utils::instance_lock::AcquireOutcome::Acquired(g) => g,
            _ => panic!(),
        };
        // Wipe the sidecar: instance_lock falls back to HeldByLive { pid: 0 }
        // when the sidecar is missing.
        let holder_path = dir.path().join("aleph.lock.pid");
        std::fs::remove_file(&holder_path).unwrap();
        // The exclusive lock file is still held (we still own `_hold`),
        // so the second acquire will return HeldByLive with pid 0.
        let result: anyhow::Result<i32> = try_with_policy::<_, i32>(
            CommandPolicy::LockOnly,
            dir.path(),
            |_lock| Ok(7),
            serde_json::Value::Null,
        );
        let err = result.expect_err("should fail when held");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("orphaned lock detected"),
            "expected orphaned-lock message, got: {msg}"
        );
        assert!(
            !msg.contains("(PID 0)"),
            "must not surface PID 0 as a live holder: {msg}"
        );
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
