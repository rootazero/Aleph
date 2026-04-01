//! Stop hooks — external commands executed before the agent loop stops.
//!
//! Hooks receive context via stdin JSON and communicate decisions via exit code:
//! - 0: allow stop
//! - 2: block stop (stdout = reason)
//! - other: hook error (logged, does not block)

use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// A registered stop hook.
pub struct StopHook {
    pub name: String,
    pub command: String,
    pub timeout: Duration,
}

impl StopHook {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

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

/// Execute all stop hooks in parallel.
pub async fn execute_stop_hooks(
    hooks: &[StopHook],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult {
    use futures::future::join_all;

    let context_json =
        serde_json::to_string(context).unwrap_or_else(|_| "{}".to_string());

    let futures: Vec<_> = hooks
        .iter()
        .map(|hook| execute_single_hook(hook, &context_json, cancel))
        .collect();

    let verdicts = join_all(futures).await;
    StopHookAggregateResult { verdicts }
}

async fn execute_single_hook(
    hook: &StopHook,
    context_json: &str,
    cancel: &CancellationToken,
) -> StopHookVerdict {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let result = tokio::select! {
        r = async {
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
                        hook_name: hook.name.clone(),
                        message: format!("failed to spawn: {e}"),
                    };
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(context_json.as_bytes()).await;
                drop(stdin);
            }

            match tokio::time::timeout(hook.timeout, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    let code = output.status.code().unwrap_or(-1);
                    match code {
                        0 => StopHookVerdict::Allow,
                        2 => {
                            let reason = String::from_utf8_lossy(&output.stdout)
                                .trim()
                                .to_string();
                            StopHookVerdict::Block {
                                reason: if reason.is_empty() {
                                    format!("hook '{}' blocked stop", hook.name)
                                } else {
                                    reason
                                },
                            }
                        }
                        _ => StopHookVerdict::Error {
                            hook_name: hook.name.clone(),
                            message: format!(
                                "exit code {code}: {}",
                                String::from_utf8_lossy(&output.stderr).trim()
                            ),
                        },
                    }
                }
                Ok(Err(e)) => StopHookVerdict::Error {
                    hook_name: hook.name.clone(),
                    message: format!("wait failed: {e}"),
                },
                Err(_) => StopHookVerdict::Error {
                    hook_name: hook.name.clone(),
                    message: "timed out".to_string(),
                },
            }
        } => r,
        _ = cancel.cancelled() => {
            StopHookVerdict::Error {
                hook_name: hook.name.clone(),
                message: "cancelled".to_string(),
            }
        }
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hook_allow() {
        let hook = StopHook::new("allow", "exit 0");
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
    }

    #[tokio::test]
    async fn test_hook_block() {
        let hook = StopHook::new("blocker", "echo 'tests not passing' && exit 2");
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("tests not passing"));
    }

    #[tokio::test]
    async fn test_hook_error_non_blocking() {
        let hook = StopHook::new("broken", "exit 1");
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
        assert_eq!(result.errors().len(), 1);
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let hook = StopHook::new("slow", "sleep 60")
            .with_timeout(Duration::from_millis(100));
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("timed out"));
    }

    #[tokio::test]
    async fn test_hook_receives_context_json() {
        let hook = StopHook::new(
            "ctx_checker",
            r#"input=$(cat); echo "$input" | grep -q "end_turn" && echo "found end_turn" && exit 2 || exit 0"#,
        );
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&[hook], &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("found end_turn"));
    }

    #[tokio::test]
    async fn test_multiple_hooks_first_block_wins() {
        let hooks = vec![
            StopHook::new("allow1", "exit 0"),
            StopHook::new("blocker", "echo 'blocked' && exit 2"),
            StopHook::new("allow2", "exit 0"),
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

    #[test]
    fn test_aggregate_no_errors() {
        let result = StopHookAggregateResult {
            verdicts: vec![StopHookVerdict::Allow, StopHookVerdict::Allow],
        };
        assert!(result.blocking_reason().is_none());
        assert!(result.errors().is_empty());
    }
}
