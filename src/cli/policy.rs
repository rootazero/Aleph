//! Declarative policy + dispatch helpers for CLI subcommands.
//!
//! Every CLI subcommand declares one of three policies and dispatches
//! through `run_no_lock` (NoLock) or `with_policy` (LockOnly / LockOrIpc,
//! filled in by Task 11).

#[derive(Debug, Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_reqwest(&self) -> reqwest::Method {
        match self {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Patch => reqwest::Method::PATCH,
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
    LockOrIpc {
        route: &'static str,
        method: HttpMethod,
    },
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

use std::path::Path;

use crate::utils::instance_lock::{self, AcquireOutcome, InstanceLock};

/// Test-friendly variant of `with_policy` that returns `Err` instead of
/// calling `std::process::exit` on lock contention. Production callers
/// should use `with_policy` which surfaces UX-friendly stderr messages.
pub fn try_with_policy<L, T>(
    policy: CommandPolicy,
    data_dir: &Path,
    local: L,
    ipc_body: serde_json::Value,
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
        CommandPolicy::LockOrIpc { route, method } => match instance_lock::try_acquire(data_dir)? {
            AcquireOutcome::Acquired(lock) => local(&lock),
            AcquireOutcome::HeldByLive { .. } | AcquireOutcome::HeldByOrphaned { .. } => {
                crate::cli::ipc_client::forward_to_server::<T>(
                    data_dir, method, route, ipc_body,
                )
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
        ).unwrap();
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
}
