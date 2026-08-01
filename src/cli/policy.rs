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
    Patch,
    Delete,
}

impl HttpMethod {
    #[must_use]
    pub const fn as_reqwest(&self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
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
        AcquireOutcome::HeldByLive { pid, lock_path } => Err(LockHeldError {
            pid: pid as u32,
            lock_path,
            orphaned: false,
        }
        .into()),
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
                    crate::cli::ipc_client::forward_to_server::<T>(
                        data_dir, method, route, ipc_body,
                    )
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
    #[cfg(not(windows))] // POSIX-only: Windows LockFileEx blocks the held-lock PID readback
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
}
