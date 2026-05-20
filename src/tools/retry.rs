//! One-shot retry backoff for tool execution.
//!
//! Per `CLAUDE.md` R10 ("dumb loop"), the harness does not select error
//! recovery strategies. This helper retries **exactly once** after
//! 100 ms when the inner `Err` reports `is_retryable()`. It does not
//! classify error types, does not back off exponentially, and does not
//! attempt more than two total invocations.
//!
//! This lights up the previously-dead `retryable` flag on `ToolError`
//! while staying within the "no policy selection" boundary.

use std::future::Future;
use std::time::Duration;

use crate::session::events::ToolOutput;
use crate::tools::service::ToolError;

/// Delay before the second attempt. Chosen to be small enough that the
/// caller does not feel a stall, but large enough that a transient
/// network/timeout retry has a real chance of succeeding.
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// Run `op` once. If it returns `Err(e)` and `e.is_retryable()`,
/// sleep 100 ms and run `op` exactly one more time. Whatever the
/// second attempt produces is returned verbatim.
pub async fn execute_with_one_shot_backoff<F, Fut>(op: F) -> Result<ToolOutput, ToolError>
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
    async fn retries_once_on_retryable_then_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = execute_with_one_shot_backoff(|| {
            let a = attempts_clone.clone();
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // Timeout is_retryable() == true.
                    Err(ToolError::Timeout {
                        name: "bash".into(),
                        elapsed_ms: 50,
                    })
                } else {
                    Ok(ok_output())
                }
            }
        })
        .await;
        assert!(result.is_ok(), "expected ok after retry: {:?}", result.err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_when_not_retryable() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let result = execute_with_one_shot_backoff(|| {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                // NotFound is_retryable() == false.
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
        let _ = execute_with_one_shot_backoff(|| {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err::<ToolOutput, _>(ToolError::Transport {
                    name: "bash".into(),
                    cause: "still down".into(),
                })
            }
        })
        .await;
        // Both attempts retry-eligible; total invocations capped at 2.
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
