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
