//! `ExtensionStopHookVerifier` — bridges `HookEvent::Stop` extension hooks
//! into the per-turn verifier chain.
//!
//! Before this gate, "can a hook veto the agent's stop?" had exactly one
//! answer: a shell command in config-TOML `[[stop_hooks]]`. Users writing
//! Claude-Code-style `hooks.json` files (and plugins shipping hook packs) had
//! no way to express the ecosystem-standard `Stop` hook — codex, hermes and
//! Claude Code all support it. This verifier closes that gap by evaluating
//! `HookEvent::Stop` interceptors at the same seam the TOML gate already
//! uses (`VerifierChain`, between Think and Act), so `src/harness/` stays
//! untouched (R10: the loop remains dumb; this is scaffolding, not
//! cognition — the *hook script* decides, we only relay its verdict).
//!
//! Semantics (Claude-Code / codex parity):
//! - hook blocks (`block:` line, `{"decision":"block"}`, `deny:` …) →
//!   `Veto` — the loop continues one more turn with the reason injected as
//!   feedback.
//! - hook halts (`prevent_continuation`, `{"continue":false}`) → `Halt` —
//!   the loop exits immediately (outranks block, same as codex `stop.rs`).
//! - anything else → `Continue` (fail-open for the stop itself).
//!
//! Anti-wedge bound: a hook that vetoes every stop would otherwise churn the
//! loop until `max_loops`. Consecutive vetoes are counted per session and
//! suppressed past [`MAX_CONSECUTIVE_STOP_VETOES`] (hermes bounds its
//! `pre_verify` nudges the same way, default 3). Hooks can see they already
//! fired via the `STOP_HOOK_ACTIVE` env / payload flag (codex's
//! `stop_hook_active`), so a well-written hook self-limits before the bound.
//!
//! Hot-reload friendly by construction: the executor snapshot is taken per
//! stop attempt (a rare event), so `hooks.json` edits apply to the very next
//! stop without daemon restarts.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::extension::hooks::{HookContext, HookExecutor, HookResult};
use crate::extension::HookEvent;
use crate::sync_primitives::Mutex;
use crate::verification::turn_verifier::{TurnVerifier, TurnVerifyContext, VerifierVerdict};

/// Ceiling on consecutive stop vetoes per session. Past it the gate logs and
/// lets the stop through — a broken (or adversarial) hook must not wedge the
/// loop into churning until `max_loops`.
const MAX_CONSECUTIVE_STOP_VETOES: u32 = 5;

/// Byte budget for the `LAST_ASSISTANT_MESSAGE` env var handed to hooks.
/// Char-boundary safe truncation; hooks needing the full text can read the
/// session transcript instead.
const LAST_MESSAGE_ENV_CAP: usize = 4096;

/// Internal decision extracted from a Stop-hook interceptor pass.
#[derive(Debug, PartialEq, Eq)]
enum StopDecision {
    /// Exit the loop immediately (claude-code `continue: false`).
    Halt(String),
    /// Force one more turn with the reason as feedback.
    Veto(String),
    /// No objection — the stop proceeds.
    Allow,
}

/// Map an aggregated `HookResult` onto a stop decision. Halt outranks veto
/// (codex: `continue:false` wins over `decision:block`).
///
/// Infrastructure failures (timeout / spawn error → `action_failed`) map to
/// `Allow`, NOT `Veto`: the interceptor executor sets `blocked` on action
/// errors (fail-closed is right for the tool gate), but the stop gate is
/// fail-open by policy — a broken script must never wedge the loop.
fn decide(hr: &HookResult) -> StopDecision {
    if hr.action_failed {
        tracing::warn!(
            reason = hr.block_reason.as_deref().unwrap_or("unknown"),
            "Stop hook action failed at the infrastructure level; allowing stop (fail-open)"
        );
        return StopDecision::Allow;
    }
    if hr.prevent_continuation {
        return StopDecision::Halt(hr.stop_message("stop halted by extension hook"));
    }
    if hr.denied || hr.blocked {
        let reason = hr
            .deny_reason
            .clone()
            .or_else(|| hr.block_reason.clone())
            .unwrap_or_else(|| "stop vetoed by extension hook".to_string());
        return StopDecision::Veto(reason);
    }
    StopDecision::Allow
}

/// Truncate at a char boundary within `cap` bytes.
fn truncate_chars(s: &str, cap: usize) -> &str {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub struct ExtensionStopHookVerifier {
    /// Consecutive-veto counts keyed by session. Entries exist only while a
    /// session is actively being vetoed (removed on Allow/Halt), so the map
    /// stays bounded by the number of concurrently-wedged sessions.
    vetoes: Mutex<HashMap<String, u32>>,
}

impl Default for ExtensionStopHookVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionStopHookVerifier {
    #[must_use]
    pub fn new() -> Self {
        Self {
            vetoes: Mutex::new(HashMap::new()),
        }
    }

    fn veto_count(&self, session: &str) -> u32 {
        self.vetoes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session)
            .copied()
            .unwrap_or(0)
    }

    fn record_veto(&self, session: &str) -> u32 {
        let mut map = self.vetoes.lock().unwrap_or_else(|e| e.into_inner());
        let count = map.entry(session.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    fn clear(&self, session: &str) {
        self.vetoes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session);
    }

    /// Core evaluation against an explicit executor — the seam unit tests
    /// use (the trait impl resolves the process-global extension manager,
    /// which tests must not depend on).
    async fn verify_with_executor(
        &self,
        executor: &HookExecutor,
        ctx: &TurnVerifyContext<'_>,
    ) -> VerifierVerdict {
        let session = ctx.session_id.unwrap_or("(no-session)");
        let prior_vetoes = self.veto_count(session);

        let mut hctx = HookContext::new(session)
            .with_env("ITERATIONS", ctx.iterations.to_string())
            .with_env("TOOL_CALLS_MADE", ctx.tool_calls_made.to_string())
            .with_env(
                "STOP_HOOK_ACTIVE",
                if prior_vetoes > 0 { "true" } else { "false" },
            );
        if let Some(text) = ctx.final_text {
            hctx = hctx.with_env(
                "LAST_ASSISTANT_MESSAGE",
                truncate_chars(text, LAST_MESSAGE_ENV_CAP).to_string(),
            );
        }

        // Observer-kind Stop hooks (kind:"observer") witness every stop
        // attempt fire-and-forget; only interceptor-kind hooks can veto/halt.
        // Without this, an observer Stop hook would register but never run
        // (the gate used to dispatch interceptors only).
        executor.execute_observers(HookEvent::Stop, &hctx).await;

        let hr = match executor.execute_interceptors(HookEvent::Stop, hctx).await {
            Ok((_ctx, hr)) => hr,
            Err(e) => {
                tracing::warn!(error = %e, "Stop hook interceptor pass failed; allowing stop");
                self.clear(session);
                return VerifierVerdict::Continue;
            }
        };

        match decide(&hr) {
            StopDecision::Halt(reason) => {
                self.clear(session);
                VerifierVerdict::Halt { reason }
            }
            StopDecision::Veto(reason) => {
                let count = self.record_veto(session);
                if count > MAX_CONSECUTIVE_STOP_VETOES {
                    tracing::warn!(
                        session,
                        count,
                        "Stop hook exceeded consecutive-veto ceiling; allowing stop"
                    );
                    // Reset the budget: the run ends here, and the NEXT run
                    // in this session deserves a fresh veto allowance — a
                    // lingering count would permanently disable the gate for
                    // the session and misreport STOP_HOOK_ACTIVE=true on a
                    // fresh run's first stop attempt.
                    self.clear(session);
                    return VerifierVerdict::Continue;
                }
                VerifierVerdict::Veto { reason }
            }
            StopDecision::Allow => {
                self.clear(session);
                VerifierVerdict::Continue
            }
        }
    }
}

#[async_trait]
impl TurnVerifier for ExtensionStopHookVerifier {

    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        // Only gate actual stop attempts; mid-turn checks pay zero cost.
        if ctx.stop_reason.is_none() {
            return VerifierVerdict::Continue;
        }
        let Some(manager) = crate::extension::try_extension_manager() else {
            return VerifierVerdict::Continue;
        };
        let executor = manager.hook_executor_snapshot().await;
        if !executor.has_hooks_for(HookEvent::Stop) {
            return VerifierVerdict::Continue;
        }
        self.verify_with_executor(&executor, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{HookAction, HookConfig, HookKind, HookPriority};
    use std::path::PathBuf;

    #[cfg_attr(not(unix), allow(dead_code))]
    fn stop_hook_config(command: &str) -> HookConfig {
        HookConfig {
            event: HookEvent::Stop,
            kind: HookKind::Interceptor,
            priority: HookPriority::default(),
            matcher: None,
            actions: vec![HookAction::Command {
                command: command.to_string(),
            }],
            plugin_name: "stop-gate-test".to_string(),
            plugin_root: PathBuf::from("/tmp"),
            handler: None,
            timeout_secs: None,
        }
    }

    fn turn_ctx<'a>(session: &'a str, stop: Option<&'a str>) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 3,
            tool_calls_made: 7,
            final_text: Some("done"),
            recent_tool_calls: &[],
            stop_reason: stop,
            session_id: Some(session),
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        }
    }

    #[test]
    fn decide_maps_block_to_veto_and_continue_false_to_halt() {
        let mut hr = HookResult::default();
        assert_eq!(decide(&hr), StopDecision::Allow);

        hr.blocked = true;
        hr.block_reason = Some("tests failing".into());
        assert_eq!(decide(&hr), StopDecision::Veto("tests failing".into()));

        // Halt outranks block (codex: continue:false wins).
        hr.prevent_continuation = true;
        hr.messages.push("budget hit".into());
        assert_eq!(decide(&hr), StopDecision::Halt("budget hit".into()));
    }

    #[test]
    fn decide_infra_failure_is_fail_open_not_veto() {
        // An interceptor ACTION error sets blocked=true (the tool gate fails
        // closed by policy), but the stop gate must fail OPEN — a broken
        // script cannot be allowed to wedge the loop.
        let hr = HookResult {
            blocked: true,
            block_reason: Some("Interceptor hook failed: Command timed out".into()),
            action_failed: true,
            ..Default::default()
        };
        assert_eq!(decide(&hr), StopDecision::Allow);
    }

    #[test]
    fn truncate_chars_is_boundary_safe() {
        let s = "日本語テキスト"; // 3-byte chars
        let t = truncate_chars(s, 7);
        assert!(t.len() <= 7);
        assert!(s.starts_with(t));
        assert_eq!(truncate_chars("short", 100), "short");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn allowing_hook_lets_stop_through() {
        let executor = HookExecutor::new(vec![stop_hook_config("exit 0")]);
        let gate = ExtensionStopHookVerifier::new();
        let verdict = gate
            .verify_with_executor(&executor, &turn_ctx("s1", Some("end_turn")))
            .await;
        assert!(verdict.is_continue());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blocking_hook_vetoes_and_sets_active_flag() {
        // First stop attempt: STOP_HOOK_ACTIVE must be false; second: true.
        // The hook blocks only when the flag is false, so the second attempt
        // passes — proving both the veto and the flag plumbing.
        let executor = HookExecutor::new(vec![stop_hook_config(
            r#"if [ "$STOP_HOOK_ACTIVE" = "false" ]; then echo 'block: not done yet'; fi"#,
        )]);
        let gate = ExtensionStopHookVerifier::new();

        let first = gate
            .verify_with_executor(&executor, &turn_ctx("s2", Some("end_turn")))
            .await;
        match first {
            VerifierVerdict::Veto { ref reason, .. } => assert_eq!(reason, "not done yet"),
            other => panic!("expected Veto, got {other:?}"),
        }

        let second = gate
            .verify_with_executor(&executor, &turn_ctx("s2", Some("end_turn")))
            .await;
        assert!(second.is_continue(), "flag-aware hook must allow round 2");
        // Counter resets on Allow, so a later stop starts fresh.
        assert_eq!(gate.veto_count("s2"), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn halting_hook_exits_loop() {
        let executor = HookExecutor::new(vec![stop_hook_config("echo 'prevent_continuation'")]);
        let gate = ExtensionStopHookVerifier::new();
        let verdict = gate
            .verify_with_executor(&executor, &turn_ctx("s3", Some("end_turn")))
            .await;
        assert!(verdict.is_halt());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn consecutive_veto_ceiling_unwedges_the_loop() {
        let executor = HookExecutor::new(vec![stop_hook_config("echo 'block: forever'")]);
        let gate = ExtensionStopHookVerifier::new();
        let ctx = turn_ctx("s4", Some("end_turn"));

        for i in 0..MAX_CONSECUTIVE_STOP_VETOES {
            let verdict = gate.verify_with_executor(&executor, &ctx).await;
            assert!(verdict.is_veto(), "veto {i} within ceiling");
        }
        // Past the ceiling the gate gives up and lets the stop through.
        let verdict = gate.verify_with_executor(&executor, &ctx).await;
        assert!(verdict.is_continue(), "ceiling must unwedge the loop");
        // ...and the counter resets, so the NEXT run in this session gets a
        // fresh veto budget instead of a permanently-disabled gate.
        assert_eq!(
            gate.veto_count("s4"),
            0,
            "ceiling breach must reset the count"
        );
        let verdict = gate.verify_with_executor(&executor, &ctx).await;
        assert!(verdict.is_veto(), "post-reset veto must be honored again");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn json_stop_decision_is_honored() {
        // Claude-Code JSON contract on the Stop event.
        let executor = HookExecutor::new(vec![stop_hook_config(
            r#"echo '{"decision":"block","reason":"coverage below 80%"}'"#,
        )]);
        let gate = ExtensionStopHookVerifier::new();
        let verdict = gate
            .verify_with_executor(&executor, &turn_ctx("s5", Some("end_turn")))
            .await;
        match verdict {
            VerifierVerdict::Veto { ref reason, .. } => {
                assert_eq!(reason, "coverage below 80%");
            }
            other => panic!("expected Veto, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_stop_reason_short_circuits() {
        // Trait-level fast path: mid-turn checks never resolve the manager.
        let gate = ExtensionStopHookVerifier::new();
        let cancel = CancellationToken::new();
        let verdict = gate.verify(&turn_ctx("s6", None), &cancel).await;
        assert!(verdict.is_continue());
    }
}
