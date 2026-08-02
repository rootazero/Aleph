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

/// Returns true iff `name` is a built-in tool that is safe to auto-retry.
///
/// A tool qualifies iff re-running with identical input has no observable
/// side effect. The answer delegates to the read-only allowlist
/// ([`crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS`]): read-only
/// ⇒ idempotent, and today no builtin is idempotent-but-mutating, so one
/// maintained list serves both the concurrency claim and this retry gate.
///
/// This replaces the former standalone `IDEMPOTENT_BUILTIN_TOOLS` list,
/// which had drifted hard from the read-only allowlist (2026-07-17): ~25
/// registered pure reads (`file_read`, `session_list`, the `desktop_ax_*`
/// family, …) were missing — losing auto-retry AND tripping the `Ask` exec
/// tier's `!idempotent` rule on pure reads, contradicting its own "read-only
/// tools stay allowed" contract — while it carried phantom (`list_tools`,
/// `search_tools`) and stale (`skill_reader` for `skill_read`) names, plus
/// `note_schema`, whose `write` action is NOT idempotent and must not be
/// exempted from the Ask tier. If a genuinely idempotent-but-mutating
/// builtin ever appears (an `mkdir -p` analogue), reintroduce a small extras
/// list here rather than polluting `READ_ONLY_TOOLS`.
///
/// Builtins only. MCP tools declare their own idempotency through the
/// server's `readOnlyHint` / `idempotentHint`, surfaced as
/// `LoopTool::is_idempotent`; extensions that declare nothing stay
/// non-idempotent. Default-deny: anything unlisted never auto-retries.
#[must_use]
pub fn is_idempotent_builtin_name(name: &str) -> bool {
    crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS.contains(&name)
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
    // `is_retryable` means "this failure was not a verdict on the call", which
    // is true of a cancellation and is why the harness must not ban it. It does
    // not mean "try again right now": the run has been stopped, so a respin
    // would sleep the backoff and fail identically.
    if matches!(e, ToolError::Cancelled { .. }) {
        return first;
    }
    if !idempotent {
        return first;
    }
    tokio::time::sleep(RETRY_DELAY).await;
    // Emit a structured event so the per-call `tool.execute` span set up by
    // `ScopedToolService::execute` carries a visible retry marker for
    // downstream tracing consumers.
    if let Err(ref e) = first {
        tracing::info!(
            "tool.retry" = true,
            "tool.error" = %e,
            "tool.delay_ms" = RETRY_DELAY.as_millis() as u64,
            "tool one-shot retry"
        );
    }
    op().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        // Consolidation regressions: registered pure reads the old standalone
        // list had drifted away from (they lost auto-retry and wrongly tripped
        // the Ask tier's `!idempotent` rule).
        assert!(is_idempotent_builtin_name("file_read"));
        assert!(is_idempotent_builtin_name("skill_read"));
        assert!(is_idempotent_builtin_name("session_list"));
        assert!(is_idempotent_builtin_name("desktop_ax_snapshot"));
        // Input-dependent read/write multiplexers stay non-idempotent — their
        // write arm is exactly what the Ask tier must keep gating.
        assert!(!is_idempotent_builtin_name("note_schema"));
        assert!(!is_idempotent_builtin_name("doctor"));
        assert!(!is_idempotent_builtin_name("file_ops"));
        assert!(!is_idempotent_builtin_name("a2a_agents"));
        // Consuming / output tools masquerading as reads must stay out:
        // retrying inbox_read can swallow messages its first (timed-out)
        // attempt already marked read; retrying heartbeat_report(notify)
        // double-messages the user.
        assert!(!is_idempotent_builtin_name("inbox_read"));
        assert!(!is_idempotent_builtin_name("heartbeat_report"));
        // Stale/phantom names must stay out.
        assert!(!is_idempotent_builtin_name("skill_reader"));
        assert!(!is_idempotent_builtin_name("list_tools"));
        assert!(!is_idempotent_builtin_name("bash_exec"));
        assert!(!is_idempotent_builtin_name("session_send"));
        assert!(!is_idempotent_builtin_name("nonexistent_tool"));
    }

    /// The allowlist must key on the tools' REGISTERED names, not module names
    /// or never-registered ghosts — otherwise the entry classifies nothing.
    #[test]
    fn idempotent_allowlist_uses_live_tool_names() {
        // Live read-only meta-tools are recognized.
        assert!(is_idempotent_builtin_name("skill_read"));
        assert!(is_idempotent_builtin_name("skill_list"));
        assert!(is_idempotent_builtin_name("tool_search"));
        assert!(is_idempotent_builtin_name("get_tool_schema"));
        // Ghost names that match no registered tool are gone.
        assert!(!is_idempotent_builtin_name("skill_reader"));
        assert!(!is_idempotent_builtin_name("list_tools"));
        assert!(!is_idempotent_builtin_name("search_tools"));
    }
}
