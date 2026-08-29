//! Stop hooks — pluggable checks executed before the agent loop stops.
//!
//! Each hook implements [`StopHookHandler`] and returns a [`StopHookVerdict`].
//! The built-in [`ShellStopHook`] runs an external shell command (`sh -c` on
//! POSIX, `cmd /C` on Windows — the same platform-aware invocation as the
//! extension hook executor in `src/extension/hooks/executor.rs`) and maps its
//! exit code:
//! - exit 0: allow stop
//! - exit 2: block stop, retry the loop (stdout = reason)
//! - exit 3: halt the loop immediately (stdout = reason)
//! - other / killed by signal / timeout: hook error (logged, does not block)
//!
//! Error semantics deliberately diverge per consumer:
//! - **harness verifier path** (`StopHookVerifier`): fail-open — a hook
//!   error never blocks the stop (a broken script must not wedge the loop).
//! - **goal objective gate** (`goal_continuation::gate_veto`): fail-closed —
//!   "gate could not be evaluated" vetoes the completion claim, because an
//!   unverifiable goal completion must not be trusted.
//!
//! `hooks.json` users get the same stop-gating via the extension `Stop`
//! event (`verification::extension_stop_gate`), evaluated right after these
//! TOML hooks in the same `VerifierChain`.

use crate::sync_primitives::Arc;
use crate::utils::no_window::NoWindow;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::config::types::StopHookConfig;

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
    /// When true, the command string is checked for shell metacharacters before
    /// being passed to `sh -c`. Per-goal gates (sourced from LLM tool args) set
    /// this to true so that injection payloads such as `;`, `|`, `$()`, etc.
    /// are rejected rather than executed.
    require_shell_safe: bool,
}

impl ShellStopHook {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            hook_name: name.into(),
            command: command.into(),
            timeout: Duration::from_secs(30),
            require_shell_safe: false,
        }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Mark this hook as requiring a shell-safe command string. Used for
    /// per-goal gates that originate from untrusted tool arguments.
    #[must_use]
    pub const fn shell_safe(mut self) -> Self {
        self.require_shell_safe = true;
        self
    }
}

#[async_trait::async_trait]
impl StopHookHandler for ShellStopHook {
    fn name(&self) -> &str {
        &self.hook_name
    }

    async fn evaluate(&self, ctx: &StopHookContext, cancel: &CancellationToken) -> StopHookVerdict {
        let context_json = serde_json::to_string(ctx).unwrap_or_else(|_| "{}".to_string());
        execute_shell_hook(self, &context_json, cancel).await
    }
}

/// Build a vector of stop-hook handlers from `config.toml [[stop_hooks]]`.
///
/// Returns `None` when the input is empty so `AgentHarnessRunner::stop_hooks`
/// can stay `None` (zero-overhead path) and the loop short-circuits past the
/// hook stage entirely; otherwise returns `Some(Arc<Vec<...>>)` ready to
/// hand to the harness.
///
/// Always emits an INFO log so harness scenario S1.4 has a passive wiring
/// proof regardless of whether any hooks are configured — but the message
/// distinguishes the disabled (no config) path from the registered
/// (`count=N`) path, so a missing config is never confused with a healthy one.
pub fn build_from_config(cfgs: &[StopHookConfig]) -> Option<Arc<Vec<Arc<dyn StopHookHandler>>>> {
    if cfgs.is_empty() {
        tracing::info!("Stop hooks: none configured (verifier stage disabled)");
        return None;
    }
    tracing::info!(count = cfgs.len(), "Stop hooks registered");
    let hooks: Vec<Arc<dyn StopHookHandler>> = cfgs
        .iter()
        .map(|c| {
            let mut h = ShellStopHook::new(&c.name, &c.command);
            if let Some(secs) = c.timeout_secs {
                h = h.with_timeout(Duration::from_secs(secs));
            }
            Arc::new(h) as Arc<dyn StopHookHandler>
        })
        .collect();
    Some(Arc::new(hooks))
}

/// Assemble the effective objective gate for a goal: the global config hooks
/// (if any) PLUS a per-goal ad-hoc [`ShellStopHook`] built from
/// `goal_gate_command` (if any). Returns `None` only when neither source is
/// present (caller then treats a `complete` claim as terminal — Round 1
/// behavior). AND semantics: the combined vector runs through
/// `execute_stop_hooks_arc`, which vetoes on the first block, so either source
/// can veto completion.
#[must_use]
pub fn effective_gate(
    global: Option<&Arc<Vec<Arc<dyn StopHookHandler>>>>,
    goal_gate_command: Option<&str>,
) -> Option<Arc<Vec<Arc<dyn StopHookHandler>>>> {
    match (global, goal_gate_command) {
        (None, None) => None,
        (Some(g), None) => Some(g.clone()),
        (g, Some(cmd)) => {
            let mut hooks: Vec<Arc<dyn StopHookHandler>> =
                g.map(|v| v.as_ref().clone()).unwrap_or_default();
            hooks.push(Arc::new(ShellStopHook::new("goal_gate", cmd).shell_safe())
                as Arc<dyn StopHookHandler>);
            Some(Arc::new(hooks))
        }
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
    Block {
        reason: String,
    },
    /// Permanent stop signal — the harness exits the loop immediately and
    /// surfaces `reason` to the user. Mirrors claude-code's
    /// `preventContinuation: true` exit-protocol. Shell hooks emit this
    /// via exit code 3 (exit 2 still maps to the retry-style `Block`).
    Halt {
        reason: String,
    },
    Error {
        hook_name: String,
        message: String,
    },
}

/// Aggregated result of all stop hooks.
#[derive(Debug)]
pub struct StopHookAggregateResult {
    pub verdicts: Vec<StopHookVerdict>,
}

impl StopHookAggregateResult {
    /// Returns the first blocking reason, if any.
    #[must_use]
    pub fn blocking_reason(&self) -> Option<&str> {
        self.verdicts.iter().find_map(|v| match v {
            StopHookVerdict::Block { reason } => Some(reason.as_str()),
            _ => None,
        })
    }

    /// Returns the first halt reason, if any. Halt outranks Block — when
    /// both are present in the same aggregate the harness must honour
    /// Halt (claude-code's `preventContinuation` semantics).
    #[must_use]
    pub fn halt_reason(&self) -> Option<&str> {
        self.verdicts.iter().find_map(|v| match v {
            StopHookVerdict::Halt { reason } => Some(reason.as_str()),
            _ => None,
        })
    }

    /// Returns all error messages.
    #[must_use]
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

/// `Arc`-parameter version of `execute_stop_hooks` — for consumers outside the
/// harness that hold guards as `Arc` (goal-loop gate, `StopHookVerifier`). Wraps
/// each `Arc` into a forwarding box, reuses the concurrent runner above, and
/// never clones hook implementations.
pub async fn execute_stop_hooks_arc(
    hooks: &[Arc<dyn StopHookHandler>],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult {
    struct ArcHook(Arc<dyn StopHookHandler>);
    #[async_trait::async_trait]
    impl StopHookHandler for ArcHook {
        fn name(&self) -> &str {
            self.0.name()
        }
        async fn evaluate(
            &self,
            ctx: &StopHookContext,
            cancel: &CancellationToken,
        ) -> StopHookVerdict {
            self.0.evaluate(ctx, cancel).await
        }
    }
    let boxed: Vec<Box<dyn StopHookHandler>> = hooks
        .iter()
        .map(|h| Box::new(ArcHook(h.clone())) as Box<dyn StopHookHandler>)
        .collect();
    execute_stop_hooks(&boxed, context, cancel).await
}

// ---------------------------------------------------------------------------
// Shell execution helper
// ---------------------------------------------------------------------------

const MAX_OUTPUT_BYTES: u64 = 64 * 1024;

/// Reject command strings that contain shell metacharacters which could be used
/// to inject additional commands when passed to `sh -c`. Alphanumeric ASCII,
/// whitespace, and common safe path/punctuation characters are allowed.
///
/// Public so the `goal` tool can enforce the SAME rule at the boundary: a
/// per-goal `gate_command` that would be rejected here is a gate that can never
/// pass judgment, and the model must learn that when it sets the gate — not
/// silently, several autonomous iterations later, at the completion claim.
#[must_use]
pub fn is_shell_safe(command: &str) -> bool {
    const SAFE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 /._-:\"'=";
    // Newline / carriage-return are already absent from `SAFE`, so the
    // single `contains` check covers them — no extra explicit comparison.
    command.chars().all(|c| SAFE.contains(c))
}

/// Build a platform-appropriate shell invocation for `command`.
///
/// POSIX uses `sh -c <command>`; Windows uses `cmd /C <command>`. This mirrors
/// the extension hook executor (`src/extension/hooks/executor.rs`) so stop
/// hooks and per-goal gates behave identically on every platform Aleph ships
/// to — Windows is a first-class target (see CLAUDE.md Windows build section),
/// where a POSIX `sh` is not guaranteed on `PATH`.
fn shell_command(command: &str) -> tokio::process::Command {
    use tokio::process::Command;
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

async fn execute_shell_hook(
    hook: &ShellStopHook,
    context_json: &str,
    cancel: &CancellationToken,
) -> StopHookVerdict {
    use tokio::io::AsyncWriteExt;

    if hook.require_shell_safe && !is_shell_safe(&hook.command) {
        return StopHookVerdict::Error {
            hook_name: hook.hook_name.clone(),
            message: format!(
                "goal gate command contains shell metacharacters: {}",
                hook.command
            ),
        };
    }

    let mut child = match shell_command(&hook.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .no_window()
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

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let mut stdin_handle = child.stdin.take();
    let result = tokio::select! {
        r = async {
            // Stdin write runs CONCURRENTLY with the output reads: a context
            // JSON larger than the pipe buffer (final_text can be arbitrarily
            // long) would otherwise deadlock against a hook that fills its
            // stdout before draining stdin — surfacing as a spurious timeout.
            //
            // `read_capped` drains past the cap so a hook printing >64KB can
            // never block on a full pipe (which would hang `wait()` into the
            // timeout and replace the REAL exit-code verdict with a spurious
            // Error). Truncation here is harmless: the decision rides the
            // exit code; stdout is only the human-readable reason.
            let (_, (stdout_buf, _), (stderr_buf, _)) = tokio::join!(
                async {
                    if let Some(mut stdin) = stdin_handle.take() {
                        if let Err(e) = stdin.write_all(context_json.as_bytes()).await {
                            tracing::debug!(error = %e, "stop hook stdin write failed");
                        }
                        drop(stdin);
                    }
                },
                async {
                    match stdout_handle {
                        Some(h) => {
                            crate::extension::hooks::read_capped(h, MAX_OUTPUT_BYTES).await
                        }
                        None => (Vec::new(), false),
                    }
                },
                async {
                    match stderr_handle {
                        Some(h) => {
                            crate::extension::hooks::read_capped(h, MAX_OUTPUT_BYTES).await
                        }
                        None => (Vec::new(), false),
                    }
                }
            );

            match child.wait().await {
                Ok(status) => {
                    match status.code() {
                        Some(0) => StopHookVerdict::Allow,
                        Some(2) => {
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
                        Some(3) => {
                            // Exit 3 = Halt — claude-code `preventContinuation`
                            // semantics. Loop exits immediately; the reason
                            // is surfaced via TerminateReason::StopHookHalt.
                            let reason = String::from_utf8_lossy(&stdout_buf)
                                .trim()
                                .to_string();
                            StopHookVerdict::Halt {
                                reason: if reason.is_empty() {
                                    format!("hook '{}' halted loop", hook.hook_name)
                                } else {
                                    reason
                                },
                            }
                        }
                        Some(code) => StopHookVerdict::Error {
                            hook_name: hook.hook_name.clone(),
                            message: format!(
                                "exit code {code}: {}",
                                String::from_utf8_lossy(&stderr_buf).trim()
                            ),
                        },
                        None => StopHookVerdict::Error {
                            hook_name: hook.hook_name.clone(),
                            message: format!(
                                "terminated by signal: {}",
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
            if let Err(e) = child.kill().await {
                tracing::debug!(error = %e, "stop hook kill after timeout failed");
            }
            // Reap the killed process. `tokio::process::Child::drop` only
            // sends SIGKILL on `kill_on_drop` — it does NOT call `waitpid`,
            // so a child killed here and dropped would persist as a zombie
            // until the parent reaps. Discard the status; the verdict is
            // already Error. (2026-08-29 verification audit.)
            let _ = child.wait().await;
            StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: "timed out".to_string(),
            }
        }
        _ = cancel.cancelled() => {
            if let Err(e) = child.kill().await {
                tracing::debug!(error = %e, "stop hook kill after cancel failed");
            }
            // Same zombie-reap rationale as the timeout arm — see comment above.
            let _ = child.wait().await;
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

    // Portable across `sh -c` and `cmd /C`: `exit 2` blocks with the default
    // reason, exercising the cross-platform shell-dispatch path on every target.
    #[tokio::test]
    async fn test_hook_block_default_reason_portable() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![shell_hook("blocker", "exit 2")];
        let ctx = StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        let reason = result.blocking_reason().expect("exit 2 must block");
        assert!(reason.contains("blocked stop"), "got: {reason}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hook_block() {
        let hooks: Vec<Box<dyn StopHookHandler>> =
            vec![shell_hook("blocker", "echo 'tests not passing' && exit 2")];
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hook_timeout() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(
            ShellStopHook::new("slow", "sleep 60").with_timeout(Duration::from_millis(100)),
        )];
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hook_receives_context_json() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![shell_hook(
            "ctx_checker",
            r#"input=$(cat); echo "$input" | grep -q "end_turn" && echo "found end_turn" && exit 2 || exit 0"#,
        )];
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_hook_cancel_kills_child() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(
            ShellStopHook::new("long_running", "sleep 60").with_timeout(Duration::from_secs(30)),
        )];
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
    fn effective_gate_combines_sources() {
        // Neither → None (Round 1 terminal behavior).
        assert!(effective_gate(None, None).is_none());
        // Global only → the global vector (length preserved).
        let global: Arc<Vec<Arc<dyn StopHookHandler>>> = Arc::new(vec![
            Arc::new(ShellStopHook::new("g", "true")) as Arc<dyn StopHookHandler>,
        ]);
        assert_eq!(effective_gate(Some(&global), None).unwrap().len(), 1);
        // Per-goal only → one hook.
        assert_eq!(effective_gate(None, Some("cargo test")).unwrap().len(), 1);
        // Both → global ⧺ per-goal.
        assert_eq!(
            effective_gate(Some(&global), Some("cargo test"))
                .unwrap()
                .len(),
            2
        );
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
        fn name(&self) -> &str {
            "always_allow"
        }
        async fn evaluate(
            &self,
            _ctx: &StopHookContext,
            _cancel: &CancellationToken,
        ) -> StopHookVerdict {
            StopHookVerdict::Allow
        }
    }

    /// A hook that blocks when iterations exceed a threshold.
    struct IterationGuardHook {
        max_iterations: usize,
    }

    #[async_trait::async_trait]
    impl StopHookHandler for IterationGuardHook {
        fn name(&self) -> &str {
            "iteration_guard"
        }
        async fn evaluate(
            &self,
            ctx: &StopHookContext,
            _cancel: &CancellationToken,
        ) -> StopHookVerdict {
            if ctx.iterations > self.max_iterations {
                StopHookVerdict::Block {
                    reason: format!(
                        "too many iterations: {} > {}",
                        ctx.iterations, self.max_iterations
                    ),
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
        assert!(result
            .blocking_reason()
            .unwrap()
            .contains("too many iterations"));
    }
}
