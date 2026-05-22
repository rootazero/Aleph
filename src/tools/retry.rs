//! One-shot retry backoff for tool execution.
//!
//! Per `CLAUDE.md` R10 ("dumb loop"), the harness does not select error
//! recovery strategies. This helper retries **exactly once** after
//! 100 ms when the inner `Err` reports `is_retryable()` AND the tool is
//! declared `idempotent`. It does not classify error types, does not back
//! off exponentially, and does not attempt more than two total invocations.
//!
//! ## Idempotency gate
//!
//! `Timeout` and `Transport` errors can fire after a side-effecting request
//! has already reached the server — re-running such a call would produce
//! duplicate side effects (double-charge, double-send). To prevent this
//! class of silent correctness bug, non-idempotent tools skip the retry
//! entirely; the LLM observes the error and decides whether to retry on
//! its next turn (R7 placement of the decision).
//!
//! Idempotency is a **static per-tool classification** mirroring
//! hermes-agent's `IDEMPOTENT_TOOL_NAMES` set. Read-only / pure-query tools
//! opt in via `is_idempotent_builtin_name` below, which also drives the
//! `ToolDefinitionMetadata.idempotent` bit surfaced via `describe()`.
//! Default-deny: any tool not on the list never auto-retries.

use std::future::Future;
use std::time::Duration;

use crate::session::events::ToolOutput;
use crate::tools::service::ToolError;

/// Delay before the second attempt. Chosen to be small enough that the
/// caller does not feel a stall, but large enough that a transient
/// network/timeout retry has a real chance of succeeding.
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// Built-in tool names that are safe to auto-retry on retryable errors.
///
/// A tool belongs here iff re-running with identical input has no
/// observable side effect — i.e. read-only, pure-query, or naturally
/// idempotent (HTTP GET, vector search, file listing, status probes).
///
/// This is the single source of truth for built-in idempotency. Adding a
/// tool to this list requires confirming the implementation has no
/// caller-visible side effects (no writes, no message sends, no remote
/// state mutation). Default-deny: anything not on this list never
/// auto-retries.
///
/// Tools served via MCP / extensions remain non-idempotent by default;
/// future work can wire per-tool metadata for MCP servers to opt in.
pub const IDEMPOTENT_BUILTIN_TOOLS: &[&str] = &[
    // Memory / recall — pure reads
    "memory_search",
    "memory_browse",
    "memory_timeline",
    "memory_explore",
    "recall_context",
    "session_search",
    "user_profile",
    // File / code search — pure reads
    "search",
    // Skill discovery — pure reads
    "skill_status",
    "skill_reader",
    // Tool introspection — pure reads
    "list_tools",
    "get_tool_schema",
    // Web — GET-only path via safe_fetch
    "web_fetch",
    // Note discovery — pure reads
    "note_orient",
    "note_schema",
];

/// Returns true iff `name` is a built-in tool that is safe to auto-retry.
pub fn is_idempotent_builtin_name(name: &str) -> bool {
    IDEMPOTENT_BUILTIN_TOOLS.contains(&name)
}

/// Run `op` once. If it returns `Err(e)` and `e.is_retryable()` AND
/// `idempotent` is true, sleep 100 ms and run `op` exactly one more time.
/// Whatever the second attempt produces is returned verbatim.
///
/// Non-idempotent tools (default) skip the retry on `Timeout`/`Transport`
/// to avoid duplicate side effects — see the module docstring.
pub async fn execute_with_one_shot_backoff<F, Fut>(
    idempotent: bool,
    op: F,
) -> Result<ToolOutput, ToolError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<ToolOutput, ToolError>>,
{
    let first: Result<ToolOutput, ToolError> = op().await;
    let Err(ref e) = first else {
        return first;
    };
    if !e.is_retryable() {
        return first;
    }
    if !idempotent {
        return first;
    }
    tokio::time::sleep(RETRY_DELAY).await;
    op().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Build a successful `ToolOutput` for assertion paths.
    fn ok_output() -> ToolOutput {
        ToolOutput {
            value: serde_json::Value::String("ok".into()),
            metadata: Default::default(),
        }
    }

    #[tokio::test]
    async fn retries_once_when_idempotent_and_retryable() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = execute_with_one_shot_backoff(true, || {
            let a = attempts_clone.clone();
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(ToolError::Timeout {
                        name: "memory_search".into(),
                        elapsed_ms: 50,
                    })
                } else {
                    Ok(ok_output())
                }
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "expected ok after retry: {:?}",
            result.err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_when_non_idempotent_even_if_retryable() {
        // The double-send guard: Timeout on a side-effecting tool may have
        // already reached the server. Re-running it could duplicate the
        // side effect (e.g. send_telegram_message). We refuse the retry.
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = execute_with_one_shot_backoff(false, || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err::<ToolOutput, _>(ToolError::Timeout {
                    name: "send_message".into(),
                    elapsed_ms: 50,
                })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "non-idempotent tools must not auto-retry"
        );
    }

    #[tokio::test]
    async fn does_not_retry_when_idempotent_but_not_retryable() {
        // NotFound is not retryable regardless of idempotency.
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = execute_with_one_shot_backoff(true, || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err::<ToolOutput, _>(ToolError::NotFound { name: "x".into() })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn caps_at_two_attempts_even_if_both_retryable() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let _ = execute_with_one_shot_backoff(true, || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err::<ToolOutput, _>(ToolError::Transport {
                    name: "search".into(),
                    cause: "still down".into(),
                })
            }
        })
        .await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn idempotent_builtin_name_lookup() {
        assert!(is_idempotent_builtin_name("memory_search"));
        assert!(is_idempotent_builtin_name("search"));
        assert!(is_idempotent_builtin_name("web_fetch"));
        assert!(!is_idempotent_builtin_name("bash_exec"));
        assert!(!is_idempotent_builtin_name("session_send"));
        assert!(!is_idempotent_builtin_name("nonexistent_tool"));
    }
}
