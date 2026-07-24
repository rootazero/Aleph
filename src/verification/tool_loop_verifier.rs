//! `ToolLoopVerifier` — structural watchdog that vetoes when the
//! model has issued N consecutive identical tool calls (same `name` +
//! `args_hash`), regardless of any narration text on the turn (closes
//! master roadmap § 1.4 P1: "stop hook only triggers when the model stops; tool_use infinite loop is not covered").
//!
//! Detection rule (deliberately conservative — false positives are
//! costly because they inject a [verifier veto] message that disrupts
//! the model):
//!   - this is a *mid-turn* turn (`ctx.stop_reason.is_none()`) — the loop
//!     is still emitting tool calls; the stop turn belongs to the stop /
//!     goal verifiers, and firing there would only re-judge stale history
//!   - `ctx.recent_tool_calls.len() >= profile.repeat_threshold`
//!   - the trailing `repeat_threshold` entries all have the same `name` and
//!     `args_hash` (identical, redundant calls — varied args reset the run)
//!
//! Two-tier escalation:
//!   - at `repeat_threshold` identical trailing calls → emit a `Veto`
//!     (`ErrorClass::Recoverable`). The harness injects it as a user message so
//!     the model sees explicit feedback and gets a chance to course-correct.
//!   - at `halt_threshold` (≥ `repeat_threshold`) identical trailing calls → emit
//!     a `Halt`. By this point the model has ignored several vetoes and is still
//!     repeating the same call with no thinking text; continuing would only burn
//!     LLM round-trips until the provider's rate limit (or a turn/stall
//!     timeout) kills the run with a confusing error. Halting deterministically
//!     here ends the unproductive loop with a clear reason instead.
//!
//! Tier 2 (same name, low-distinctness args): when the *entire* history window
//! is the same tool `name`, the `(name, args_hash)` pairs revisit a SMALL set
//! (distinctness < `profile.novelty_min`), and the turn carries no narration
//! text, emit a `Veto` (steer). This catches a thrash the identical-args check
//! is blind to (e.g. re-reading three reference files round and round). Unlike
//! the previous behaviour, Tier-2 now emits `Veto` (not `Halt`): the harness
//! injects feedback and, only if the model ignores `steer_max` of them, a
//! wrap-up grace turn fires. High-distinctness fan-out (e.g. 8 distinct
//! `web_fetch` URLs) has distinctness ≥ `novelty_min` and therefore NEVER
//! reaches the Tier-2 branch — this is the key fix for the parallel news-fetch
//! false positive.
//!
//! All tiers are pure structural checks over `(name, args_hash)` and the
//! presence/absence of text — no model reasoning, so this stays scaffolding
//! (R10-safe), never a completion judge.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::ErrorClass;
use crate::verification::turn_verifier::{
    ToolCallSummary, TurnVerifier, TurnVerifyContext, VerifierVerdict, TOOL_HISTORY_WINDOW,
};

pub struct ToolLoopVerifier;

impl ToolLoopVerifier {
    /// Construct the verifier. Detection thresholds (`repeat_threshold`,
    /// `halt_threshold`, etc.) are read from `ctx.robustness_profile` at
    /// `verify` time — see `ModelRobustnessProfile` and `TurnVerifyContext`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Number of distinct `(name, args_hash)` pairs in the window. A low ratio
/// of distinct/total means the model is revisiting a small set of calls
/// (thrash); a high ratio means genuine fan-out / exploration.
fn distinct_count(calls: &[ToolCallSummary]) -> usize {
    let mut seen: Vec<(&str, u64)> = Vec::with_capacity(calls.len());
    for c in calls {
        let key = (c.name.as_str(), c.args_hash);
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen.len()
}

/// Length of the trailing run of calls identical (same `name` + `args_hash`)
/// to the most recent one. `0` for an empty slice.
fn trailing_repeat_run(calls: &[ToolCallSummary]) -> usize {
    let Some(last) = calls.last() else {
        return 0;
    };
    calls
        .iter()
        .rev()
        .take_while(|c| c.name == last.name && c.args_hash == last.args_hash)
        .count()
}

/// Length of the trailing run of calls sharing the most recent call's `name`,
/// **ignoring `args_hash`**. `0` for an empty slice. Used by Tier 2 to catch a
/// same-tool thrash whose arguments keep changing (so the identical-args
/// [`trailing_repeat_run`] never accumulates).
fn trailing_same_name_run(calls: &[ToolCallSummary]) -> usize {
    let Some(last) = calls.last() else {
        return 0;
    };
    calls
        .iter()
        .rev()
        .take_while(|c| c.name == last.name)
        .count()
}

impl Default for ToolLoopVerifier {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl TurnVerifier for ToolLoopVerifier {
    fn name(&self) -> &str {
        "tool_loop"
    }

    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        // Death-loop detection is a *mid-turn* concern (see original rationale).
        if ctx.stop_reason.is_some() {
            return VerifierVerdict::Continue;
        }
        let profile = ctx.robustness_profile;
        if ctx.recent_tool_calls.len() < profile.repeat_threshold {
            return VerifierVerdict::Continue;
        }
        let run = trailing_repeat_run(ctx.recent_tool_calls);

        // Tier 1 — identical (name + args_hash) consecutive calls. Fires
        // regardless of narration: an exact-repeat is never productive.
        if run >= profile.repeat_threshold {
            let tool = &ctx.recent_tool_calls[ctx.recent_tool_calls.len() - 1].name;
            if run >= profile.halt_threshold && profile.halt_threshold > profile.repeat_threshold {
                return VerifierVerdict::Halt {
                    reason: format!(
                        "tool '{tool}' invoked {run} consecutive times with identical arguments \
                         despite repeated feedback — terminating an unproductive loop",
                    ),
                    class: ErrorClass::Recoverable,
                };
            }
            return VerifierVerdict::Veto {
                reason: format!(
                    "tool '{tool}' invoked {run} consecutive times with identical arguments — \
                     try a different approach or summarize what you've found",
                ),
                class: ErrorClass::Recoverable,
            };
        }

        // Tier 2 — same tool NAME fills the window but arguments revisit a
        // SMALL set (low distinctness) AND no narration. This is a thrash
        // (e.g. re-reading 3 files round and round). UNLIKE the old code this
        // now emits a `Veto` (steer), not a terminal `Halt`: the harness
        // injects feedback and, only if the model ignores `steer_max` of them,
        // its veto-cap path fires a wrap-up grace turn. High-distinctness
        // fan-out (e.g. 8 distinct web_fetch URLs) has distinctness == 1.0,
        // which is >= novelty_min, so it never reaches here.
        let same_name_run = trailing_same_name_run(ctx.recent_tool_calls);
        let has_text = ctx.final_text.is_some_and(|t| !t.trim().is_empty());
        let distinct = distinct_count(ctx.recent_tool_calls);
        let distinctness = distinct as f32 / ctx.recent_tool_calls.len() as f32;
        let silent_ok = !profile.silence_required || !has_text;
        if same_name_run >= TOOL_HISTORY_WINDOW && distinctness < profile.novelty_min && silent_ok {
            let tool = &ctx.recent_tool_calls[ctx.recent_tool_calls.len() - 1].name;
            return VerifierVerdict::Veto {
                reason: format!(
                    "tool '{tool}' invoked {same_name_run} times cycling a small set of arguments \
                     ({distinct} distinct) with no narration — stop and summarize, or take a \
                     different approach",
                ),
                class: ErrorClass::Recoverable,
            };
        }

        VerifierVerdict::Continue
    }
}

#[cfg(test)]
mod profile_wiring_tests {
    use crate::verification::turn_verifier::{ToolCallSummary, TurnVerifyContext};
    use crate::verification::ModelRobustnessProfile;

    fn ctx_with<'a>(
        calls: &'a [ToolCallSummary],
        profile: ModelRobustnessProfile,
        text: Option<&'a str>,
    ) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 0,
            tool_calls_made: 0,
            final_text: text,
            recent_tool_calls: calls,
            stop_reason: None,
            session_id: None,
            robustness_profile: profile,
        }
    }

    #[test]
    fn context_carries_profile() {
        let calls: Vec<ToolCallSummary> = Vec::new();
        let c = ctx_with(&calls, ModelRobustnessProfile::conservative(), None);
        assert_eq!(c.robustness_profile.repeat_threshold, 5);
    }
}

#[cfg(test)]
mod distinctness_tests {
    use super::*;
    use crate::verification::turn_verifier::{ToolCallSummary, TurnVerifyContext};
    use crate::verification::ModelRobustnessProfile;
    use tokio_util::sync::CancellationToken;

    fn call(name: &str, args: u64) -> ToolCallSummary {
        ToolCallSummary {
            name: name.to_string(),
            args_hash: args,
        }
    }

    fn ctx<'a>(
        calls: &'a [ToolCallSummary],
        profile: ModelRobustnessProfile,
        text: Option<&'a str>,
    ) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 0,
            tool_calls_made: 0,
            final_text: text,
            recent_tool_calls: calls,
            stop_reason: None,
            session_id: None,
            robustness_profile: profile,
        }
    }

    // THE NEWS CASE: 8 web_fetch, all distinct URLs → fan-out → Continue.
    #[tokio::test]
    async fn high_distinctness_fanout_passes() {
        let calls: Vec<_> = (0..8).map(|i| call("web_fetch", i)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v
            .verify(
                &ctx(&calls, ModelRobustnessProfile::conservative(), None),
                &CancellationToken::new(),
            )
            .await;
        assert!(
            verdict.is_continue(),
            "distinct fan-out must not trip: {verdict:?}"
        );
    }

    // THRASH: 3 files cycling, 8 same-name silent → Tier-2 → VETO (was Halt).
    #[tokio::test]
    async fn low_distinctness_thrash_steers_not_halts() {
        let calls: Vec<_> = (0..8).map(|i| call("file_read", (i % 3) as u64)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v
            .verify(
                &ctx(&calls, ModelRobustnessProfile::conservative(), None),
                &CancellationToken::new(),
            )
            .await;
        assert!(
            verdict.is_veto(),
            "silent thrash must steer (Veto), not Halt: {verdict:?}"
        );
    }

    // THRASH WITH NARRATION + silence_required → Continue (legit exploration).
    #[tokio::test]
    async fn thrash_with_narration_passes_when_silence_required() {
        let calls: Vec<_> = (0..8).map(|i| call("file_read", (i % 3) as u64)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v
            .verify(
                &ctx(
                    &calls,
                    ModelRobustnessProfile::conservative(),
                    Some("Comparing the three files..."),
                ),
                &CancellationToken::new(),
            )
            .await;
        assert!(
            verdict.is_continue(),
            "narrated exploration must pass: {verdict:?}"
        );
    }

    // TIER-1 identical run at repeat_threshold → Veto.
    #[tokio::test]
    async fn identical_run_vetoes() {
        let calls: Vec<_> = (0..5).map(|_| call("grep", 42)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v
            .verify(
                &ctx(&calls, ModelRobustnessProfile::conservative(), None),
                &CancellationToken::new(),
            )
            .await;
        assert!(verdict.is_veto(), "identical run should veto: {verdict:?}");
    }

    // TIER-1 identical run at full window → Halt (dead loop).
    #[tokio::test]
    async fn identical_run_full_window_halts() {
        let calls: Vec<_> = (0..8).map(|_| call("grep", 42)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v
            .verify(
                &ctx(&calls, ModelRobustnessProfile::conservative(), None),
                &CancellationToken::new(),
            )
            .await;
        assert!(
            verdict.is_halt(),
            "identical full-window run should halt: {verdict:?}"
        );
    }
}
