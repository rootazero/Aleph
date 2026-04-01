//! Stop hooks — pluggable checks executed before the agent loop stops.
//!
//! Each hook implements [`StopHookHandler`] and returns a [`StopHookVerdict`].
//! The built-in [`ShellStopHook`] runs an external shell command:
//! - exit 0: allow stop
//! - exit 2: block stop (stdout = reason)
//! - other: hook error (logged, does not block)

use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for stop hook implementations.
#[async_trait::async_trait]
pub trait StopHookHandler: Send + Sync {
    /// Human-readable name of this hook (used in logs / error messages).
    fn name(&self) -> &str;

    /// Evaluate whether the agent loop should stop.
    async fn evaluate(&self, ctx: &StopHookContext, cancel: &CancellationToken) -> StopHookVerdict;
}

// ---------------------------------------------------------------------------
// ShellStopHook (formerly StopHook)
// ---------------------------------------------------------------------------

/// A stop hook that runs an external shell command.
pub struct ShellStopHook {
    pub hook_name: String,
    pub command: String,
    pub timeout: Duration,
}

impl ShellStopHook {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            hook_name: name.into(),
            command: command.into(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait::async_trait]
impl StopHookHandler for ShellStopHook {
    fn name(&self) -> &str {
        &self.hook_name
    }

    async fn evaluate(&self, ctx: &StopHookContext, cancel: &CancellationToken) -> StopHookVerdict {
        let context_json =
            serde_json::to_string(ctx).unwrap_or_else(|_| "{}".to_string());
        execute_shell_hook(self, &context_json, cancel).await
    }
}

// ---------------------------------------------------------------------------
// Context & Verdict types
// ---------------------------------------------------------------------------

/// Context passed to stop hooks via stdin as JSON.
#[derive(Serialize)]
pub struct StopHookContext {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub stop_reason: String,
}

/// Result of a single hook execution.
#[derive(Debug)]
pub enum StopHookVerdict {
    Allow,
    Block { reason: String },
    Error { hook_name: String, message: String },
}

/// Aggregated result of all stop hooks.
#[derive(Debug)]
pub struct StopHookAggregateResult {
    pub verdicts: Vec<StopHookVerdict>,
}

impl StopHookAggregateResult {
    /// Returns the first blocking reason, if any.
    pub fn blocking_reason(&self) -> Option<&str> {
        self.verdicts.iter().find_map(|v| match v {
            StopHookVerdict::Block { reason } => Some(reason.as_str()),
            _ => None,
        })
    }

    /// Returns all error messages.
    pub fn errors(&self) -> Vec<(&str, &str)> {
        self.verdicts
            .iter()
            .filter_map(|v| match v {
                StopHookVerdict::Error { hook_name, message } => {
                    Some((hook_name.as_str(), message.as_str()))
                }
                _ => None,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Execute all stop hooks in parallel.
pub async fn execute_stop_hooks(
    hooks: &[Box<dyn StopHookHandler>],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult {
    use futures::future::join_all;

    let futures: Vec<_> = hooks
        .iter()
        .map(|hook| hook.evaluate(context, cancel))
        .collect();

    let verdicts = join_all(futures).await;
    StopHookAggregateResult { verdicts }
}

// ---------------------------------------------------------------------------
// Shell execution helper
// ---------------------------------------------------------------------------

async fn execute_shell_hook(
    hook: &ShellStopHook,
    context_json: &str,
    cancel: &CancellationToken,
) -> StopHookVerdict {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&hook.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: format!("failed to spawn: {e}"),
            };
        }
    };

    use tokio::io::AsyncReadExt;
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let result = tokio::select! {
        r = async {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(context_json.as_bytes()).await;
                drop(stdin);
            }

            let (stdout_buf, stderr_buf) = tokio::join!(
                async {
                    let mut buf = Vec::new();
                    if let Some(ref mut h) = stdout_handle {
                        let _ = h.read_to_end(&mut buf).await;
                    }
                    buf
                },
                async {
                    let mut buf = Vec::new();
                    if let Some(ref mut h) = stderr_handle {
                        let _ = h.read_to_end(&mut buf).await;
                    }
                    buf
                }
            );

            match child.wait().await {
                Ok(status) => {
                    let code = status.code().unwrap_or(-1);
                    match code {
                        0 => StopHookVerdict::Allow,
                        2 => {
                            let reason = String::from_utf8_lossy(&stdout_buf)
                                .trim()
                                .to_string();
                            StopHookVerdict::Block {
                                reason: if reason.is_empty() {
                                    format!("hook '{}' blocked stop", hook.hook_name)
                                } else {
                                    reason
                                },
                            }
                        }
                        _ => StopHookVerdict::Error {
                            hook_name: hook.hook_name.clone(),
                            message: format!(
                                "exit code {code}: {}",
                                String::from_utf8_lossy(&stderr_buf).trim()
                            ),
                        },
                    }
                }
                Err(e) => StopHookVerdict::Error {
                    hook_name: hook.hook_name.clone(),
                    message: format!("wait failed: {e}"),
                },
            }
        } => r,
        _ = tokio::time::sleep(hook.timeout) => {
            let _ = child.kill().await;
            StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: "timed out".to_string(),
            }
        }
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: "cancelled".to_string(),
            }
        }
    };

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a boxed ShellStopHook.
    fn shell_hook(name: &str, cmd: &str) -> Box<dyn StopHookHandler> {
        Box::new(ShellStopHook::new(name, cmd))
    }

    #[tokio::test]
    async fn test_hook_allow() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![shell_hook("allow", "exit 0")];
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
    }

    #[tokio::test]
    async fn test_hook_block() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![
            shell_hook("blocker", "echo 'tests not passing' && exit 2"),
        ];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("tests not passing"));
    }

    #[tokio::test]
    async fn test_hook_error_non_blocking() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![shell_hook("broken", "exit 1")];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
        assert_eq!(result.errors().len(), 1);
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![
            Box::new(ShellStopHook::new("slow", "sleep 60").with_timeout(Duration::from_millis(100))),
        ];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("timed out"));
    }

    #[tokio::test]
    async fn test_hook_receives_context_json() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![
            shell_hook(
                "ctx_checker",
                r#"input=$(cat); echo "$input" | grep -q "end_turn" && echo "found end_turn" && exit 2 || exit 0"#,
            ),
        ];
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("found end_turn"));
    }

    #[tokio::test]
    async fn test_multiple_hooks_first_block_wins() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![
            shell_hook("allow1", "exit 0"),
            shell_hook("blocker", "echo 'blocked' && exit 2"),
            shell_hook("allow2", "exit 0"),
        ];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("blocked"));
    }

    #[tokio::test]
    async fn test_hook_cancel_kills_child() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![
            Box::new(ShellStopHook::new("long_running", "sleep 60").with_timeout(Duration::from_secs(30))),
        ];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let start = std::time::Instant::now();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(5),
            "Cancel should be fast, took {:?}",
            elapsed
        );
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("cancelled"));
    }

    #[test]
    fn test_aggregate_no_errors() {
        let result = StopHookAggregateResult {
            verdicts: vec![StopHookVerdict::Allow, StopHookVerdict::Allow],
        };
        assert!(result.blocking_reason().is_none());
        assert!(result.errors().is_empty());
    }

    // --- Trait-based tests (custom in-process implementations) ---

    /// A hook that always allows stopping.
    struct AlwaysAllowHook;

    #[async_trait::async_trait]
    impl StopHookHandler for AlwaysAllowHook {
        fn name(&self) -> &str { "always_allow" }
        async fn evaluate(&self, _ctx: &StopHookContext, _cancel: &CancellationToken) -> StopHookVerdict {
            StopHookVerdict::Allow
        }
    }

    /// A hook that blocks when iterations exceed a threshold.
    struct IterationGuardHook {
        max_iterations: usize,
    }

    #[async_trait::async_trait]
    impl StopHookHandler for IterationGuardHook {
        fn name(&self) -> &str { "iteration_guard" }
        async fn evaluate(&self, ctx: &StopHookContext, _cancel: &CancellationToken) -> StopHookVerdict {
            if ctx.iterations > self.max_iterations {
                StopHookVerdict::Block {
                    reason: format!("too many iterations: {} > {}", ctx.iterations, self.max_iterations),
                }
            } else {
                StopHookVerdict::Allow
            }
        }
    }

    #[tokio::test]
    async fn test_custom_hook_always_allow() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(AlwaysAllowHook)];
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 100,
            tool_calls_made: 50,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
    }

    #[tokio::test]
    async fn test_custom_hook_iteration_guard_blocks() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![
            Box::new(AlwaysAllowHook),
            Box::new(IterationGuardHook { max_iterations: 3 }),
        ];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 10,
            tool_calls_made: 20,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert!(result.blocking_reason().unwrap().contains("too many iterations"));
    }
}
